//! Streaming vs full-clip integration test (pause-split segments).
//!
//! Needs the v3 model installed and fixture audio (`JISUNO_FIXTURES` or
//! `/tmp/fixtures`). Ignored by default; run with:
//! `cargo test --release -p jisuno-asr --test streaming -- --ignored --nocapture`
//! Skips quietly when the model or fixtures are missing (e.g. Linux CI).

use std::path::{Path, PathBuf};

use jisuno_asr::transcribe::{StreamConfig, StreamingTranscriber};

fn fixtures_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("JISUNO_FIXTURES") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    let fallback = PathBuf::from("/tmp/fixtures");
    fallback.is_dir().then_some(fallback)
}

fn pairs(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut wavs: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "wav") && path.with_extension("txt").exists() {
                wavs.push(path);
            }
        }
    }
    wavs.sort();
    wavs.into_iter()
        .map(|wav| {
            let stem = wav
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            (stem, wav)
        })
        .collect()
}

fn normalize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn levenshtein(a: &[String], b: &[String]) -> usize {
    let m = b.len();
    let mut prev: Vec<usize> = (0..=m).collect();
    for (i, x) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, y) in b.iter().enumerate() {
            cur.push(
                (prev[j] + usize::from(x != y))
                    .min(prev[j + 1] + 1)
                    .min(cur[j] + 1),
            );
        }
        prev = cur;
    }
    prev[m]
}

fn wer(reference: &str, hypothesis: &str) -> f64 {
    let (r, h) = (normalize(reference), normalize(hypothesis));
    if r.is_empty() {
        return if h.is_empty() { 0.0 } else { 1.0 };
    }
    levenshtein(&r, &h) as f64 / r.len() as f64
}

/// Feed `pcm` in 100 ms pushes (no sleeping) and return the transcript.
fn stream_all(session: &mut dyn StreamingTranscriber, pcm: &[f32]) -> anyhow::Result<String> {
    session.reset();
    for chunk in pcm.chunks(1600) {
        session.push(chunk)?;
    }
    session.finish()
}

#[test]
#[ignore]
fn streaming_matches_full_clip_within_guard() {
    let model = jisuno_asr::setup::Model::ParakeetV3Int8;
    if !model.is_ready() {
        eprintln!("model not installed; skipping streaming integration test");
        return;
    }
    let Some(dir) = fixtures_dir() else {
        eprintln!("no fixtures dir; skipping streaming integration test");
        return;
    };
    let pairs = pairs(&dir);
    if pairs.is_empty() {
        eprintln!("no fixture pairs in {}; skipping", dir.display());
        return;
    }
    let mut full_t = jisuno_asr::transcribe::load(model).expect("load full transcriber");
    let mut stream_t =
        jisuno_asr::transcribe::load_streaming_with_config(model, StreamConfig::default())
            .expect("load streaming session");
    let mut full_sum = 0.0;
    let mut stream_sum = 0.0;
    for (stem, wav) in &pairs {
        let reference = std::fs::read_to_string(wav.with_extension("txt")).expect("read ref");
        let pcm = jisuno_audio::audio::load_wav_16k_mono(wav).expect("load wav");
        let mut parts = Vec::new();
        full_t
            .transcribe(&pcm, &mut |s: &str| parts.push(s.to_string()))
            .expect("full decode");
        let full_hyp = parts.join(" ").trim().to_string();
        let stream_hyp = stream_all(&mut *stream_t, &pcm).expect("streaming decode");
        let (wf, ws) = (wer(&reference, &full_hyp), wer(&reference, &stream_hyp));
        full_sum += wf;
        stream_sum += ws;
        println!("{stem}: full WER {wf:.3} stream WER {ws:.3}");
        println!(
            "  full:   {}",
            full_hyp.chars().take(120).collect::<String>()
        );
        println!(
            "  stream: {}",
            stream_hyp.chars().take(120).collect::<String>()
        );
    }
    let n = pairs.len() as f64;
    let (full_mean, stream_mean) = (full_sum / n, stream_sum / n);
    println!("mean: full {full_mean:.4} stream {stream_mean:.4}");
    assert!(
        stream_mean <= full_mean + 0.01,
        "streaming WER {stream_mean:.4} exceeds full-clip {full_mean:.4} + 0.01"
    );
}

#[test]
#[ignore]
fn streaming_is_deterministic_across_runs() {
    let model = jisuno_asr::setup::Model::ParakeetV3Int8;
    if !model.is_ready() {
        eprintln!("model not installed; skipping streaming determinism test");
        return;
    }
    let Some(dir) = fixtures_dir() else {
        eprintln!("no fixtures dir; skipping streaming determinism test");
        return;
    };
    let wav = pairs(&dir).into_iter().next().map(|(_, w)| w);
    let Some(wav) = wav else {
        eprintln!("no fixture wav; skipping streaming determinism test");
        return;
    };
    let pcm = jisuno_audio::audio::load_wav_16k_mono(&wav).expect("load wav");
    let mut session =
        jisuno_asr::transcribe::load_streaming_with_config(model, StreamConfig::default())
            .expect("load streaming session");
    let mut first: Option<String> = None;
    for i in 0..10 {
        let text = stream_all(&mut *session, &pcm).expect("streaming decode");
        println!("run {i}: {}", text.chars().take(100).collect::<String>());
        match &first {
            Some(want) => assert_eq!(&text, want, "run {i} differs: nondeterministic"),
            None => first = Some(text),
        }
    }
}

#[test]
#[ignore]
fn streaming_handles_edge_inputs() {
    let model = jisuno_asr::setup::Model::ParakeetV3Int8;
    if !model.is_ready() {
        eprintln!("model not installed; skipping streaming edge-case test");
        return;
    }
    let mut session =
        jisuno_asr::transcribe::load_streaming_with_config(model, StreamConfig::default())
            .expect("load streaming session");
    // Empty input: empty transcript, no hang.
    session.push(&[]).expect("empty push");
    assert_eq!(session.partial(), None);
    assert_eq!(session.finish().expect("empty finish"), "");
    // Shorter than any pause: whole-clip path at finish, no hang.
    let pcm = vec![0.3; 8000];
    session.push(&pcm).expect("push");
    let _ = session.finish().expect("finish");
    // Reset reuse: same input twice gives the same output.
    let a = stream_all(&mut *session, &pcm).expect("first decode");
    let b = stream_all(&mut *session, &pcm).expect("second decode");
    assert_eq!(a, b);
}
