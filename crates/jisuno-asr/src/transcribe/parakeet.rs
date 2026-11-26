//! Parakeet-TDT inference via ONNX Runtime (`ort`).
//!
//! Pipeline: `nemo128.onnx` turns the waveform into a 128-bin mel
//! spectrogram, `encoder.int8.onnx` encodes it, and the combined
//! `decoder_joint.int8.onnx` runs the predictor + joint network for greedy
//! Token-and-Duration-Transducer (TDT) decoding.
//!
//! The greedy decode (duration jumps, blank handling) follows the reference
//! implementation in `jason-ni/parakeet-rs`. The difference here is that the
//! predictor and joint are **fused** into one `decoder_joint` session (the
//! `istupakov` / PalatineVision export the `setup` command downloads), so each
//! step is a single session call.
//!
//! ## Model I/O names
//!
//! The input/output tensor names below match the istupakov/PalatineVision
//! ONNX export. If you bring a model with different names, inspect it with a
//! tool like [Netron](https://netron.app) and adjust the `const`s — that's the
//! only thing that's export-specific; the decode logic is not.

use std::path::Path;
use std::sync::mpsc::{Receiver, Sender};

use anyhow::{anyhow, Context};
use ndarray::{s, ArrayD, ArrayViewD, IxDyn};
use ort::execution_providers::CPUExecutionProvider;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::{Tensor, TensorRef};

use super::{StreamingTranscriber, Transcriber};

// --- Preprocessor (nemo128.onnx) I/O ---
const PRE_WAVE_IN: &str = "waveforms";
const PRE_WAVE_LEN_IN: &str = "waveforms_lens";
const PRE_FEAT_OUT: &str = "features";

// --- Encoder (encoder.int8.onnx) I/O ---
const ENC_SIGNAL_IN: &str = "audio_signal";
const ENC_LEN_IN: &str = "length";
const ENC_OUT: &str = "outputs";

// --- Decoder+Joint (decoder_joint.int8.onnx) I/O ---
const DJ_ENC_IN: &str = "encoder_outputs";
const DJ_TARGETS_IN: &str = "targets";
const DJ_TARGET_LEN_IN: &str = "target_length";
const DJ_STATE1_IN: &str = "input_states_1";
const DJ_STATE2_IN: &str = "input_states_2";
const DJ_LOGITS_OUT: &str = "outputs";
const DJ_STATE1_OUT: &str = "output_states_1";
const DJ_STATE2_OUT: &str = "output_states_2";

/// Number of TDT duration classes (durations 0..=4). The joint output is
/// `[token_logits .. duration_logits]`; this is the size of the trailing
/// duration block, used to split the two.
const NUM_DURATIONS: usize = 5;

/// LSTM predictor state shape `[layers, batch, hidden]`.
const STATE_SHAPE: [usize; 3] = [2, 1, 640];

/// Safety cap on non-blank symbols emitted at a single encoder frame, to
/// guarantee the decode loop terminates.
const MAX_SYMBOLS_PER_FRAME: usize = 10;

/// Sample rate every entry point consumes.
const SAMPLE_RATE: u32 = 16_000;

/// Energy-frame length for the streaming pause detector (20 ms).
const FRAME_SAMPLES: usize = 320;

/// Subword marker prefix that denotes the start of a new word (U+2581).
const WORD_PREFIX: char = '▁';

pub struct ParakeetTranscriber {
    preprocessor: Session,
    encoder: Session,
    decoder_joint: Session,
    vocab: Vec<String>,
}

impl ParakeetTranscriber {
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        let preprocessor = build_session(&dir.join("nemo128.onnx"))?;
        let encoder = build_session(&dir.join("encoder.int8.onnx"))?;
        let decoder_joint = build_session(&dir.join("decoder_joint.int8.onnx"))?;
        let vocab = load_vocab(&dir.join("vocab.txt"))?;
        Ok(Self {
            preprocessor,
            encoder,
            decoder_joint,
            vocab,
        })
    }

    /// Waveform -> mel features `[1, 128, T]`.
    fn extract_features(&mut self, pcm: &[f32]) -> anyhow::Result<ArrayD<f32>> {
        let audio = ArrayViewD::from_shape(IxDyn(&[1, pcm.len()]), pcm)
            .context("shaping pcm for preprocessor")?;
        let inputs = ort::inputs![
            PRE_WAVE_IN => TensorRef::from_array_view(audio.view())?,
            PRE_WAVE_LEN_IN => Tensor::from_array(([1], vec![pcm.len() as i64].into_boxed_slice()))?,
        ];
        let outputs = self.preprocessor.run(inputs)?;
        let feats: ArrayViewD<f32> = outputs
            .get(PRE_FEAT_OUT)
            .ok_or_else(|| missing(PRE_FEAT_OUT, "preprocessor"))?
            .try_extract_array()?;
        Ok(feats.to_owned())
    }

    /// Mel features -> encoder output `[1, D, T']` (channels-first).
    fn encode(&mut self, feats: &ArrayD<f32>) -> anyhow::Result<ArrayD<f32>> {
        let length = feats.shape()[2] as i64;
        let inputs = ort::inputs![
            ENC_SIGNAL_IN => TensorRef::from_array_view(feats.view())?,
            ENC_LEN_IN => Tensor::from_array(([1], vec![length].into_boxed_slice()))?,
        ];
        let outputs = self.encoder.run(inputs)?;
        let enc: ArrayViewD<f32> = outputs
            .get(ENC_OUT)
            .ok_or_else(|| missing(ENC_OUT, "encoder"))?
            .try_extract_array()?;
        Ok(enc.to_owned())
    }

    /// One predictor+joint step over a single encoder frame.
    ///
    /// Returns the joint logits (length `vocab + NUM_DURATIONS`) and the next
    /// LSTM states.
    fn decoder_joint_step(
        &mut self,
        enc_frame: &ArrayD<f32>,
        label: i32,
        state1: &ArrayD<f32>,
        state2: &ArrayD<f32>,
    ) -> anyhow::Result<(Vec<f32>, ArrayD<f32>, ArrayD<f32>)> {
        let inputs = ort::inputs![
            DJ_ENC_IN => TensorRef::from_array_view(enc_frame.view())?,
            DJ_TARGETS_IN => Tensor::from_array(([1, 1], vec![label].into_boxed_slice()))?,
            DJ_TARGET_LEN_IN => Tensor::from_array(([1], vec![1i32].into_boxed_slice()))?,
            DJ_STATE1_IN => TensorRef::from_array_view(state1.view())?,
            DJ_STATE2_IN => TensorRef::from_array_view(state2.view())?,
        ];
        let outputs = self.decoder_joint.run(inputs)?;

        let logits: Vec<f32> = outputs
            .get(DJ_LOGITS_OUT)
            .ok_or_else(|| missing(DJ_LOGITS_OUT, "decoder_joint"))?
            .try_extract_array::<f32>()?
            .iter()
            .copied()
            .collect();
        let next1: ArrayD<f32> = outputs
            .get(DJ_STATE1_OUT)
            .ok_or_else(|| missing(DJ_STATE1_OUT, "decoder_joint"))?
            .try_extract_array::<f32>()?
            .to_owned();
        let next2: ArrayD<f32> = outputs
            .get(DJ_STATE2_OUT)
            .ok_or_else(|| missing(DJ_STATE2_OUT, "decoder_joint"))?
            .try_extract_array::<f32>()?
            .to_owned();
        Ok((logits, next1, next2))
    }
}

/// True when `JISUNO_PROFILE` is set: `transcribe` prints per-stage
/// milliseconds to stderr. Debug-only instrumentation for the perf work.
fn profiling() -> bool {
    std::env::var_os("JISUNO_PROFILE").is_some()
}

/// Carried TDT greedy-decoder state. Decoding is causal in the encoder
/// frames given this state, so decoding frames incrementally as they arrive
/// (streaming) yields exactly the same words as decoding them all at once.
#[derive(Debug)]
struct DecoderState {
    /// Last emitted token id (predictor input); starts at blank.
    label: i32,
    state1: ArrayD<f32>,
    state2: ArrayD<f32>,
    /// Next encoder frame to visit.
    frame: usize,
    /// Non-blank symbols emitted at `frame` without advancing.
    symbols_at_frame: usize,
    /// Words whose boundary has been hit.
    words: Vec<String>,
    /// Encoder frame index at which each word in `words` was flushed
    /// (boundary detected here; parallel to `words`).
    ends: Vec<usize>,
    /// In-progress word.
    cur: String,
}

impl ParakeetTranscriber {
    fn blank_label(&self) -> i32 {
        self.vocab.len().saturating_sub(1) as i32
    }

    fn fresh_decoder_state(&self) -> DecoderState {
        DecoderState {
            label: self.blank_label(),
            state1: ArrayD::zeros(IxDyn(&STATE_SHAPE)),
            state2: ArrayD::zeros(IxDyn(&STATE_SHAPE)),
            frame: 0,
            symbols_at_frame: 0,
            words: Vec::new(),
            ends: Vec::new(),
            cur: String::new(),
        }
    }

    /// Greedy TDT decode over `enc` (`[1, D, T']`, channels-first),
    /// resuming from `st` and advancing it past every frame. Returns the
    /// joint steps taken. Every step matches the reference streaming loop:
    /// blank-run fast-forward, duration jumps, and the per-frame symbol cap
    /// live here in one place so they can be reviewed against the reference
    /// without chasing helpers.
    fn decode_range(&mut self, enc: &ArrayD<f32>, st: &mut DecoderState) -> anyhow::Result<usize> {
        let n_frames = enc.shape()[2];
        if n_frames == 0 {
            return Ok(0);
        }
        // Blank is conventionally the last token class. TDT appends
        // NUM_DURATIONS duration classes after the token classes; logits =
        // [token..., blank, duration...].
        let mut steps = 0usize;
        while st.frame < n_frames {
            steps += 1;
            // Channels-first frame: [1, D, 1]. No copy on slice; the to_owned
            // below materializes just this frame for the joint step.
            let frame: ArrayD<f32> = enc
                .slice(s![.., .., st.frame..=st.frame])
                .to_owned()
                .into_dyn();

            let (logits, next1, next2) =
                self.decoder_joint_step(&frame, st.label, &st.state1, &st.state2)?;

            if logits.len() <= NUM_DURATIONS {
                return Err(anyhow!(
                    "decoder_joint produced {} logits, expected > {NUM_DURATIONS}",
                    logits.len()
                ));
            }
            let token_classes = logits.len() - NUM_DURATIONS;
            let blank = token_classes - 1;

            let token = argmax(&logits[..token_classes]);
            let duration = argmax(&logits[token_classes..]);

            if token != blank {
                // Buffer flushed words locally: the closure must not borrow
                // `st` while `st.cur` is borrowed as the emit target.
                let mut flushed = Vec::new();
                emit_token(
                    self.vocab.get(token).map(String::as_str),
                    &mut st.cur,
                    &mut |w: &str| flushed.push(w.to_string()),
                );
                // Each flushed word ends where its boundary was detected.
                for _ in 0..flushed.len() {
                    st.ends.push(st.frame);
                }
                st.words.extend(flushed);
                st.label = token as i32;
                st.state1 = next1;
                st.state2 = next2;
                st.symbols_at_frame += 1;
            }

            let mut step = duration;
            if token == blank && step == 0 {
                step = 1;
            }
            if step > 0 {
                st.frame += step;
                st.symbols_at_frame = 0;
            } else if st.symbols_at_frame >= MAX_SYMBOLS_PER_FRAME {
                st.frame += 1;
                st.symbols_at_frame = 0;
            }
        }
        Ok(steps)
    }
}

impl Transcriber for ParakeetTranscriber {
    /// Byte-faithful port of the reference streaming loop, via
    /// [`ParakeetTranscriber::decode_range`]. `on_segment` receives each
    /// decoded word as its boundary (a space piece or the final flush) is
    /// hit.
    fn transcribe(&mut self, pcm: &[f32], on_segment: &mut dyn FnMut(&str)) -> anyhow::Result<()> {
        if pcm.is_empty() {
            return Ok(());
        }
        let profile = profiling();
        let t0 = std::time::Instant::now();
        let feats = self.extract_features(pcm)?;
        let t1 = std::time::Instant::now();
        let enc = self.encode(&feats)?;
        let t2 = std::time::Instant::now();

        let mut st = self.fresh_decoder_state();
        let steps = self.decode_range(&enc, &mut st)?;

        for word in &st.words {
            on_segment(word);
        }
        if !st.cur.is_empty() {
            on_segment(&st.cur);
        }
        if profile {
            let secs = pcm.len() as f64 / 16_000.0;
            let n_frames = enc.shape()[2];
            eprintln!(
                "[parakeet-profile] audio={secs:.2}s mel={:.1}ms enc={:.1}ms dec={:.1}ms frames={n_frames} steps={steps}",
                t1.duration_since(t0).as_secs_f64() * 1000.0,
                t2.duration_since(t1).as_secs_f64() * 1000.0,
                t2.elapsed().as_secs_f64() * 1000.0,
            );
        }
        Ok(())
    }
}

/// Append a decoded subword token to the current word, flushing the previous
/// word as a segment when a new word boundary (`▁`) is reached.
fn emit_token(token: Option<&str>, word: &mut String, on_segment: &mut dyn FnMut(&str)) {
    let Some(tok) = token else { return };
    if let Some(rest) = tok.strip_prefix(WORD_PREFIX) {
        if !word.is_empty() {
            on_segment(word);
            word.clear();
        }
        word.push_str(rest);
    } else {
        word.push_str(tok);
    }
}

/// Index of the maximum value in a slice (first on ties). Returns 0 for empty.
fn argmax(values: &[f32]) -> usize {
    let mut best = 0usize;
    let mut best_v = f32::NEG_INFINITY;
    for (i, &v) in values.iter().enumerate() {
        if v > best_v {
            best_v = v;
            best = i;
        }
    }
    best
}

/// Pause-split config for streaming transcription.
///
/// While audio arrives, the session watches frame energy and cuts the
/// utterance into pause-bounded segments: when a pause of `pause_secs`
/// follows at least `min_speech_secs` of speech since the last cut, it cuts
/// at the middle of the pause. Each segment (plus `pad_secs` of surrounding
/// silence each side) is decoded on a background worker thread with a fresh
/// predictor state, so `push` stays cheap and `finish` only decodes the tail
/// since the last cut. Each decode is a natural utterance, which is what
/// keeps streaming WER at full-clip level: the mel frontend normalizes per
/// utterance and the encoder attends over the whole input, so arbitrary
/// fixed-window splices diverge (see PERF.md).
///
/// If no pause appears for `max_segment_secs`, the session force-cuts at
/// the lowest-energy 20 ms frame of the last 2 s, so continuous speech
/// still makes progress. No predictor state crosses segments.
#[derive(Debug, Clone, Copy)]
pub struct StreamConfig {
    /// Pause length that triggers a cut, in seconds.
    pub pause_secs: f32,
    /// Speech required since the last cut before a pause may cut, in seconds.
    pub min_speech_secs: f32,
    /// Force a cut after this much audio without a cut, in seconds.
    pub max_segment_secs: f32,
    /// Silence padding decoded around each segment, in seconds.
    pub pad_secs: f32,
    /// Frame RMS above which silence becomes speech (enter threshold).
    pub speech_threshold: f32,
    /// Frame RMS below which speech becomes silence (exit threshold).
    pub silence_threshold: f32,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            pause_secs: 0.3,
            min_speech_secs: 1.0,
            max_segment_secs: 8.0,
            pad_secs: 0.1,
            speech_threshold: 0.02,
            silence_threshold: 0.01,
        }
    }
}

/// Resolved whole-sample/frame counts for 16 kHz audio and 20 ms frames.
#[derive(Debug, Clone, Copy)]
struct StreamPlan {
    /// Samples per energy frame (320).
    frame: usize,
    /// Consecutive silent frames that form a pause.
    pause_frames: usize,
    /// Speech frames required since the last cut before a pause may cut.
    min_speech_frames: usize,
    /// Frames without a cut before a forced cut.
    max_segment_frames: usize,
    /// Frames searched for the forced-cut minimum (last 2 s).
    search_frames: usize,
    /// Segment padding in samples.
    pad_samples: usize,
    /// Enter-speech RMS.
    speech_hi: f32,
    /// Exit-to-silence RMS.
    silence_lo: f32,
}

impl StreamConfig {
    /// Resolve to whole counts. Errors on non-positive spans, a maximum
    /// that leaves no room for a pause cut after the minimum speech, or a
    /// degenerate threshold pair.
    fn resolve(&self) -> anyhow::Result<StreamPlan> {
        for (name, v) in [
            ("pause_secs", self.pause_secs),
            ("min_speech_secs", self.min_speech_secs),
            ("max_segment_secs", self.max_segment_secs),
        ] {
            if v.is_nan() || v <= 0.0 {
                return Err(anyhow!("stream {name} must be > 0 s"));
            }
        }
        if self.pad_secs.is_nan() || self.pad_secs < 0.0 {
            return Err(anyhow!("stream pad_secs must be >= 0 s"));
        }
        if self.silence_threshold.is_nan()
            || self.speech_threshold.is_nan()
            || self.silence_threshold <= 0.0
            || self.speech_threshold < self.silence_threshold
        {
            return Err(anyhow!("stream thresholds need 0 < silence <= speech"));
        }
        let frames =
            |secs: f32| ((secs * SAMPLE_RATE as f32) / FRAME_SAMPLES as f32).round() as usize;
        let pause_frames = frames(self.pause_secs).max(1);
        let min_speech_frames = frames(self.min_speech_secs).max(1);
        let max_segment_frames = frames(self.max_segment_secs).max(1);
        if max_segment_frames <= min_speech_frames + pause_frames {
            return Err(anyhow!(
                "stream max_segment_secs must leave room for a pause cut after min_speech_secs"
            ));
        }
        Ok(StreamPlan {
            frame: FRAME_SAMPLES,
            pause_frames,
            min_speech_frames,
            max_segment_frames,
            search_frames: frames(2.0).max(1),
            pad_samples: (self.pad_secs * SAMPLE_RATE as f32).round() as usize,
            speech_hi: self.speech_threshold,
            silence_lo: self.silence_threshold,
        })
    }
}

/// Sample-count pause detector: splits audio at middles of pauses.
///
/// Pure function of the samples fed so far — no wall clock, so back-to-back
/// pushes behave identically to real-time ones. Positions are absolute
/// sample indices from the start of the stream.
#[derive(Debug)]
struct PauseSplitter {
    plan: StreamPlan,
    /// RMS of every completed 20 ms frame since stream start.
    frame_rms: Vec<f32>,
    /// Samples of the current partial frame.
    partial: Vec<f32>,
    /// Frame index containing the last cut (segment starts here).
    seg_start_frame: usize,
    in_speech: bool,
    silence_run: usize,
    speech_since_cut: usize,
}

impl PauseSplitter {
    fn new(plan: StreamPlan) -> Self {
        Self {
            plan,
            frame_rms: Vec::new(),
            partial: Vec::new(),
            seg_start_frame: 0,
            in_speech: false,
            silence_run: 0,
            speech_since_cut: 0,
        }
    }

    /// Feed samples; returns absolute sample indices where cuts were found.
    fn feed(&mut self, samples: &[f32]) -> Vec<usize> {
        let mut cuts = Vec::new();
        for &s in samples {
            self.partial.push(s);
            if self.partial.len() == self.plan.frame {
                let rms = jisuno_logic::vad::rms(&self.partial);
                self.partial.clear();
                if let Some(cut) = self.push_frame(rms) {
                    cuts.push(cut);
                }
            }
        }
        cuts
    }

    fn push_frame(&mut self, rms: f32) -> Option<usize> {
        let p = self.plan;
        // Hysteresis: between the thresholds, keep the previous state.
        if self.in_speech {
            if rms < p.silence_lo {
                self.in_speech = false;
            }
        } else if rms > p.speech_hi {
            self.in_speech = true;
        }
        if self.in_speech {
            self.speech_since_cut += 1;
            self.silence_run = 0;
        } else {
            self.silence_run += 1;
        }
        self.frame_rms.push(rms);
        let framed = self.frame_rms.len();
        if self.silence_run >= p.pause_frames && self.speech_since_cut >= p.min_speech_frames {
            // Cut at the middle of the pause run. The pause floor is silence
            // on both sides, so the segment padding fits inside it.
            let pause_start = framed - self.silence_run;
            let mid = pause_start + self.silence_run / 2;
            let cut = mid * p.frame + p.frame / 2;
            self.mark_cut_frame(mid);
            return Some(cut);
        }
        if framed - self.seg_start_frame >= p.max_segment_frames {
            // Forced cut at the middle of the lowest-energy recent frame (a
            // stop closure or word boundary in continuous speech).
            let lo = framed
                .saturating_sub(p.search_frames)
                .max(self.seg_start_frame);
            let mut best = lo;
            for i in lo + 1..framed {
                if self.frame_rms[i] < self.frame_rms[best] {
                    best = i;
                }
            }
            let cut = best * p.frame + p.frame / 2;
            self.mark_cut_frame(best + 1);
            return Some(cut);
        }
        None
    }

    fn mark_cut_frame(&mut self, frame: usize) {
        self.seg_start_frame = frame;
        self.speech_since_cut = 0;
        self.silence_run = 0;
        self.in_speech = false;
    }

    /// Forget stream state after an explicit cut (used by `finish`, which
    /// treats the end of audio as an implicit cut).
    fn mark_cut(&mut self) {
        self.mark_cut_frame(self.frame_rms.len());
    }

    fn reset(&mut self) {
        self.frame_rms.clear();
        self.partial.clear();
        self.mark_cut();
    }
}

/// Join pause-bounded segment texts into one transcript.
///
/// Segments after the first are continuations, not new sentences: when the
/// text so far does not end with `.`, `?` or `!`, a leading Titlecase word
/// is treated as the model's sentence-case artifact and lowercased — unless
/// it is `I` or an acronym (second letter uppercase). This is a heuristic:
/// a genuine proper noun after a comma is lowercased too. Empty segments
/// are skipped; single spaces join.
fn join_segments(segments: &[String]) -> String {
    let mut out = String::new();
    for seg in segments.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if out.is_empty() {
            out.push_str(seg);
            continue;
        }
        let continuation = !matches!(out.chars().last(), Some('.' | '?' | '!'));
        let mut seg = seg.to_string();
        if continuation {
            decapitalize_first(&mut seg);
        }
        out.push(' ');
        out.push_str(&seg);
    }
    out
}

/// Lowercase the first letter unless it is `I` or starts an acronym.
fn decapitalize_first(s: &mut String) {
    let first_len = s.split_whitespace().next().map(str::len).unwrap_or(0);
    if first_len == 0 {
        return;
    }
    let mut chars = s.char_indices();
    let (i0, c0) = match chars.next() {
        Some(x) => x,
        None => return,
    };
    if !c0.is_uppercase() {
        return;
    }
    let lower = c0.to_lowercase().to_string();
    // Single-letter word: keep `I`, lowercase the rest (`A store` →
    // `a store`).
    if first_len == c0.len_utf8() {
        if &s[..first_len] != "I" {
            s.replace_range(..first_len, &lower);
        }
        return;
    }
    // Titlecase start (`Alvarez`, `The`) is the sentence-case artifact; an
    // uppercase second letter (`FBI`) is a genuine acronym.
    if let Some((_, c1)) = chars.next() {
        if c1.is_lowercase() {
            s.replace_range(i0..i0 + c0.len_utf8(), &lower);
        }
    }
}

/// Work item for the decode thread.
enum StreamReq {
    /// Decode a pause-bounded segment; report its text when done.
    Segment { gen: u64, audio: Vec<f32> },
    /// Decode the trailing audio, then reply with the full transcript.
    Finish {
        gen: u64,
        tail: Vec<f32>,
        reply: Sender<String>,
    },
    /// Drop buffered segments (generation change).
    Reset { gen: u64 },
    /// Exit the thread.
    Stop,
}

/// Decode whole audio to a single trimmed string. Segments are natural
/// utterances, so this matches full-clip decode of the same samples.
fn decode_text(t: &mut ParakeetTranscriber, pcm: &[f32]) -> String {
    if pcm.is_empty() {
        return String::new();
    }
    let mut parts = Vec::new();
    match t.transcribe(pcm, &mut |s: &str| parts.push(s.to_string())) {
        Ok(()) => parts.join(" ").trim().to_string(),
        Err(e) => {
            eprintln!("[jisuno-asr] streaming segment decode failed: {e:#}");
            String::new()
        }
    }
}

/// Decode loop owning the sessions. Segments decode with a fresh predictor
/// state each, in queue order; completed texts go back over `done_tx`
/// tagged with their generation. Stale generations are dropped by the
/// session.
fn stream_worker(
    mut t: ParakeetTranscriber,
    req_rx: Receiver<StreamReq>,
    done_tx: Sender<(u64, String)>,
) {
    let mut segments: Vec<String> = Vec::new();
    let mut gen = 0u64;
    for req in req_rx {
        match req {
            StreamReq::Segment { gen: g, audio } => {
                if g != gen {
                    continue;
                }
                let text = decode_text(&mut t, &audio);
                segments.push(text.clone());
                if done_tx.send((g, text)).is_err() {
                    break;
                }
            }
            StreamReq::Finish {
                gen: g,
                tail,
                reply,
            } => {
                if g != gen {
                    let _ = reply.send(String::new());
                    continue;
                }
                if !tail.is_empty() {
                    let text = decode_text(&mut t, &tail);
                    segments.push(text.clone());
                    // The reply carries the whole transcript; still report
                    // the tail over `done_tx` so the session's segment list
                    // stays in sync with the worker's.
                    if done_tx.send((g, text)).is_err() {
                        let _ = reply.send(String::new());
                        break;
                    }
                }
                let _ = reply.send(join_segments(&segments));
            }
            StreamReq::Reset { gen: g } => {
                gen = g;
                segments.clear();
            }
            StreamReq::Stop => break,
        }
    }
}

/// Incremental Parakeet session: while audio arrives it cuts pause-bounded
/// segments and decodes them on a background worker thread, so `push` only
/// buffers audio and runs the energy gate. `finish` decodes just the tail
/// since the last cut and joins everything. One model stays in memory,
/// owned by the worker. See [`StreamConfig`] and [`StreamingTranscriber`].
pub struct ParakeetStreamingSession {
    plan: StreamPlan,
    splitter: PauseSplitter,
    /// All audio since the last `reset` (2 min dictation cap bounds this at
    /// ~7.7 MB of f32).
    audio: Vec<f32>,
    /// Start sample of the segment currently being accumulated.
    seg_start: usize,
    /// Finished segment texts, in order (drained from the worker).
    finished: Vec<String>,
    done_rx: Receiver<(u64, String)>,
    req_tx: Option<Sender<StreamReq>>,
    worker: Option<std::thread::JoinHandle<()>>,
    gen: u64,
}

impl ParakeetStreamingSession {
    /// Load the model from `dir` (same files as [`ParakeetTranscriber`])
    /// with the given pause-split config, and start the decode thread.
    pub fn load(dir: &Path, config: StreamConfig) -> anyhow::Result<Self> {
        let plan = config.resolve()?;
        let inner = ParakeetTranscriber::load(dir)?;
        let (req_tx, req_rx) = std::sync::mpsc::channel::<StreamReq>();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<(u64, String)>();
        let worker = std::thread::Builder::new()
            .name("jisuno-stream-decode".to_string())
            .spawn(move || stream_worker(inner, req_rx, done_tx))
            .context("spawning streaming decode thread")?;
        Ok(Self {
            plan,
            splitter: PauseSplitter::new(plan),
            audio: Vec::new(),
            seg_start: 0,
            finished: Vec::new(),
            done_rx,
            req_tx: Some(req_tx),
            worker: Some(worker),
            gen: 0,
        })
    }

    fn send(&self, req: StreamReq) -> anyhow::Result<()> {
        match &self.req_tx {
            Some(tx) => tx.send(req).context("streaming decode thread gone"),
            None => Err(anyhow!("streaming session closed")),
        }
    }

    /// Move newly completed segment texts into `finished`, dropping stale
    /// generations left behind by `reset`.
    fn drain_done(&mut self) {
        while let Ok((g, text)) = self.done_rx.try_recv() {
            if g == self.gen {
                self.finished.push(text);
            }
        }
    }
}

impl Drop for ParakeetStreamingSession {
    fn drop(&mut self) {
        if let Some(tx) = self.req_tx.take() {
            let _ = tx.send(StreamReq::Stop);
        }
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

impl StreamingTranscriber for ParakeetStreamingSession {
    fn push(&mut self, pcm_16k: &[f32]) -> anyhow::Result<()> {
        if pcm_16k.is_empty() {
            return Ok(());
        }
        self.audio.extend_from_slice(pcm_16k);
        for cut in self.splitter.feed(pcm_16k) {
            let start = self.seg_start.saturating_sub(self.plan.pad_samples);
            let end = (cut + self.plan.pad_samples).min(self.audio.len());
            let segment = self.audio[start..end].to_vec();
            self.send(StreamReq::Segment {
                gen: self.gen,
                audio: segment,
            })?;
            self.seg_start = cut;
        }
        self.drain_done();
        Ok(())
    }

    fn partial(&self) -> Option<String> {
        let text = join_segments(&self.finished);
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    fn finish(&mut self) -> anyhow::Result<String> {
        if self.audio.is_empty() {
            return Ok(String::new());
        }
        self.drain_done();
        // Tail since the last cut; skip the encode when it holds no speech
        // (the same gate the splitter applies).
        let tail = &self.audio[self.seg_start..];
        let send_tail = if tail.is_empty() || jisuno_logic::vad::rms(tail) < self.plan.silence_lo {
            Vec::new()
        } else {
            tail.to_vec()
        };
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        self.send(StreamReq::Finish {
            gen: self.gen,
            tail: send_tail,
            reply: tx,
        })?;
        // The reply carries the whole transcript; drain again so `finished`
        // (and a later `partial`) stays in sync with the worker's list,
        // including the tail text the worker reported over `done_tx` first.
        let _ = rx.recv().context("streaming decode thread gone")?;
        self.drain_done();
        self.seg_start = self.audio.len();
        self.splitter.mark_cut();
        Ok(join_segments(&self.finished))
    }

    fn reset(&mut self) {
        self.gen += 1;
        let _ = self.send(StreamReq::Reset { gen: self.gen });
        self.audio.clear();
        self.splitter.reset();
        self.seg_start = 0;
        self.finished.clear();
        self.drain_done();
    }
}

/// Build a CPU ONNX session with graph optimizations enabled.
///
/// Threading (measured on the reference Mac, MacBook Air M1/8 GB — see
/// `crates/jisuno-asr/PERF.md`): 2 intra-op threads is the sweet spot for
/// the int8 encoder (5 s clip: 351 ms vs 573 ms single-threaded; 3–4
/// threads are slower again, 8 far slower — the matmuls are memory-bandwidth
/// bound). `deterministic_compute` keeps the multi-threaded int8 reductions
/// bit-stable, so greedy TDT decoding stays byte-identical run to run
/// (verified: 10/10 identical decodes on five fixtures, hypotheses
/// byte-equal to the single-threaded baseline) at no measurable speed cost.
/// Re-run that 10x determinism check if you touch threading: one flipped
/// duration argmax shifts every following frame alignment and the
/// token-conditioned predictor diverges, garbling the whole utterance.
///
/// The CPU arena allocator is off: ORT's arena holds ~0.5 GB past session
/// drop, and disabling it cuts warm RSS by a third at identical decode
/// speed (see PERF.md, experiment 5).
fn build_session(path: &Path) -> anyhow::Result<Session> {
    Session::builder()
        .context("creating ort session builder")?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .with_intra_threads(2)?
        .with_deterministic_compute(true)?
        .with_execution_providers([CPUExecutionProvider::default()
            .with_arena_allocator(false)
            .build()])?
        .commit_from_file(path)
        .with_context(|| format!("loading model {}", path.display()))
}

/// Load `vocab.txt`: one token per line, taking the first whitespace field
/// (some exports append an index column).
fn load_vocab(path: &Path) -> anyhow::Result<Vec<String>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading vocab {}", path.display()))?;
    let vocab: Vec<String> = text
        .lines()
        .map(|line| line.split_whitespace().next().unwrap_or("").to_string())
        .collect();
    if vocab.is_empty() {
        return Err(anyhow!("vocab {} is empty", path.display()));
    }
    Ok(vocab)
}

fn missing(name: &str, model: &str) -> anyhow::Error {
    anyhow!("{model} output '{name}' not found — does this ONNX export use different tensor names?")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argmax_picks_largest() {
        assert_eq!(argmax(&[0.1, 0.9, 0.3]), 1);
        assert_eq!(argmax(&[]), 0);
        assert_eq!(argmax(&[2.0, 2.0]), 0); // first on ties
    }

    #[test]
    fn emit_streams_on_word_boundary() {
        let mut out: Vec<String> = Vec::new();
        let mut word = String::new();
        {
            let mut push = |s: &str| out.push(s.to_string());
            emit_token(Some("▁hel"), &mut word, &mut push);
            emit_token(Some("lo"), &mut word, &mut push);
            emit_token(Some("▁world"), &mut word, &mut push);
        }
        // "hello" flushed when "▁world" starts; "world" still buffered.
        assert_eq!(out, vec!["hello"]);
        assert_eq!(word, "world");
    }

    #[test]
    fn stream_config_resolves_to_whole_frames() {
        let plan = StreamConfig::default().resolve().expect("defaults resolve");
        assert_eq!(plan.frame, 320);
        assert_eq!(plan.pause_frames, 15);
        assert_eq!(plan.min_speech_frames, 50);
        assert_eq!(plan.max_segment_frames, 400);
        assert_eq!(plan.search_frames, 100);
        assert_eq!(plan.pad_samples, 1600);
        assert_eq!((plan.speech_hi, plan.silence_lo), (0.02, 0.01));
    }

    #[test]
    fn stream_config_rejects_bad_values() {
        let bad = [
            StreamConfig {
                pause_secs: 0.0,
                ..Default::default()
            },
            StreamConfig {
                pause_secs: f32::NAN,
                ..Default::default()
            },
            StreamConfig {
                min_speech_secs: -1.0,
                ..Default::default()
            },
            // No room for a pause cut after the minimum speech.
            StreamConfig {
                min_speech_secs: 8.0,
                ..Default::default()
            },
            StreamConfig {
                max_segment_secs: 1.0,
                ..Default::default()
            },
            StreamConfig {
                pad_secs: -0.1,
                ..Default::default()
            },
            // Degenerate threshold pairs (silence must be positive and not
            // above speech for the hysteresis to make sense).
            StreamConfig {
                silence_threshold: 0.0,
                ..Default::default()
            },
            StreamConfig {
                speech_threshold: 0.005,
                ..Default::default()
            },
        ];
        for c in bad {
            assert!(c.resolve().is_err(), "accepted {c:?}");
        }
        assert!(StreamConfig {
            pause_secs: 0.5,
            min_speech_secs: 0.5,
            max_segment_secs: 4.0,
            ..Default::default()
        }
        .resolve()
        .is_ok());
    }

    /// Loud constant signal (deterministic frame energy).
    fn loud(seconds: f32) -> Vec<f32> {
        vec![0.3; (seconds * SAMPLE_RATE as f32).round() as usize]
    }

    /// Digital silence.
    fn quiet(seconds: f32) -> Vec<f32> {
        vec![0.0; (seconds * SAMPLE_RATE as f32).round() as usize]
    }

    /// Constant signal between the hysteresis thresholds.
    fn mid_level(seconds: f32) -> Vec<f32> {
        vec![0.015; (seconds * SAMPLE_RATE as f32).round() as usize]
    }

    fn concat(parts: &[Vec<f32>]) -> Vec<f32> {
        parts.concat()
    }

    /// Feed in odd-sized pushes so partial energy frames always span push
    /// boundaries; returns all cuts found.
    fn feed_all(splitter: &mut PauseSplitter, pcm: &[f32]) -> Vec<usize> {
        let mut cuts = Vec::new();
        for chunk in pcm.chunks(1000) {
            cuts.extend(splitter.feed(chunk));
        }
        cuts
    }

    fn default_splitter() -> PauseSplitter {
        PauseSplitter::new(StreamConfig::default().resolve().expect("defaults resolve"))
    }

    #[test]
    fn splitter_cuts_at_middle_of_pause() {
        let mut s = default_splitter();
        // 1.5 s speech, 0.5 s pause, 1.5 s speech. The pause spans frames
        // 75..100; the run hits 15 silent frames at frame 90, so the cut
        // lands at frame 82 + half a frame = sample 26400.
        let pcm = concat(&[loud(1.5), quiet(0.5), loud(1.5)]);
        assert_eq!(feed_all(&mut s, &pcm), vec![26400]);
    }

    #[test]
    fn splitter_waits_for_min_speech() {
        let mut s = default_splitter();
        // 0.5 s of speech is not enough to cut (needs 1.0 s): the first
        // pause passes silently, and the cut lands in the trailing pause.
        let pcm = concat(&[loud(0.5), quiet(0.5), loud(1.5), quiet(0.5)]);
        assert_eq!(feed_all(&mut s, &pcm), vec![42400]);
    }

    #[test]
    fn splitter_hysteresis_holds_between_thresholds() {
        let mut s = default_splitter();
        // The 0.4 s mid-level stretch (RMS 0.015, between the 0.01/0.02
        // thresholds) must not break the speech run: without hysteresis it
        // would count as 20 silent frames and cut here.
        let pcm = concat(&[loud(1.5), mid_level(0.4), loud(1.5), quiet(0.4)]);
        assert_eq!(feed_all(&mut s, &pcm), vec![56800]);
    }

    #[test]
    fn splitter_force_cuts_continuous_speech() {
        let mut s = default_splitter();
        // 9 s with no pause: one forced cut when 400 frames (8 s) are
        // buffered. All frames tie on energy, so the first searched frame
        // (index 300) wins: cut at 300 * 320 + 160.
        let pcm = loud(9.0);
        assert_eq!(feed_all(&mut s, &pcm), vec![96160]);
    }

    #[test]
    fn splitter_force_cut_picks_quietest_frame() {
        let mut s = default_splitter();
        // 8 s of signal with one silent frame at index 380: the forced cut
        // lands in the middle of that frame.
        let mut pcm = loud(8.0);
        for x in &mut pcm[380 * 320..381 * 320] {
            *x = 0.0;
        }
        assert_eq!(feed_all(&mut s, &pcm), vec![380 * 320 + 160]);
    }

    #[test]
    fn splitter_empty_and_short_audio_have_no_cuts() {
        let mut s = default_splitter();
        assert!(feed_all(&mut s, &[]).is_empty());
        assert!(feed_all(&mut s, &loud(0.5)).is_empty());
    }

    #[test]
    fn splitter_cuts_each_pause() {
        let mut s = default_splitter();
        let pcm = concat(&[loud(2.0), quiet(0.5), loud(2.0), quiet(0.5), loud(2.0)]);
        assert_eq!(feed_all(&mut s, &pcm), vec![34400, 74400]);
    }

    #[test]
    fn join_segments_basic() {
        assert_eq!(join_segments(&[]), "".to_string());
        assert_eq!(
            join_segments(&["hello".to_string(), "".to_string(), "world".to_string()]),
            "hello world".to_string()
        );
    }

    #[test]
    fn join_segments_lowercases_continuations() {
        assert_eq!(
            join_segments(&["Please schedule".to_string(), "A meeting".to_string()]),
            "Please schedule a meeting".to_string()
        );
        assert_eq!(
            join_segments(&["hello,".to_string(), "World".to_string()]),
            "hello, world".to_string()
        );
        assert_eq!(
            join_segments(&["go".to_string(), "now".to_string()]),
            "go now".to_string()
        );
        assert_eq!(
            join_segments(&["count".to_string(), "42 things".to_string()]),
            "count 42 things".to_string()
        );
    }

    #[test]
    fn join_segments_keeps_sentence_starts() {
        for end in [".", "?", "!"] {
            let first = format!("End of story{end}");
            assert_eq!(
                join_segments(&[first, "Next begins".to_string()]),
                format!("End of story{end} Next begins")
            );
        }
    }

    #[test]
    fn join_segments_keeps_i_and_acronyms() {
        assert_eq!(
            join_segments(&["they said".to_string(), "I am".to_string()]),
            "they said I am".to_string()
        );
        assert_eq!(
            join_segments(&["went with".to_string(), "FBI agents".to_string()]),
            "went with FBI agents".to_string()
        );
    }

    #[test]
    fn decapitalize_first_handles_edge_cases() {
        // Documented limitation: a genuine proper noun after a comma is
        // lowercased too — the heuristic cannot tell it from sentence case.
        let mut s = "Alvarez".to_string();
        decapitalize_first(&mut s);
        assert_eq!(s, "alvarez".to_string());
        let mut s = "A".to_string();
        decapitalize_first(&mut s);
        assert_eq!(s, "a".to_string());
        let mut s = "".to_string();
        decapitalize_first(&mut s);
        assert_eq!(s, "".to_string());
    }
}
