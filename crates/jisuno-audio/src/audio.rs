//! Audio helpers: the target sample rate, mono downmixing, resampling to
//! 16 kHz, and loading WAV files (for `--wav` and tests).
//!
//! The Parakeet model expects **16 kHz mono f32** in
//! `-1.0..=1.0`. Microphones rarely run at 16 kHz, so we resample whatever
//! the device gives us.
//!
//! Downsampling (e.g. 48 kHz → 16 kHz) is anti-aliased: a windowed-sinc
//! low-pass filter removes energy above the target Nyquist *before* the
//! samples are decimated. Skipping that step folds high frequencies back into
//! the speech band, which measurably corrupts what the recognizer hears — the
//! previous bare linear interpolator did exactly that. The filter is a short
//! hand-rolled FIR, so no extra dependency is pulled in.

use std::f32::consts::PI;
use std::path::Path;

/// Sample rate every backend expects.
pub const TARGET_RATE: u32 = 16_000;

/// FIR length for the anti-aliasing low-pass. Odd so it has a center tap;
/// long enough for a usable transition band at speech rates, short enough to
/// stay cheap even when re-run on a growing buffer for streaming partials.
const LOWPASS_TAPS: usize = 63;

/// Average interleaved multi-channel samples down to mono.
pub fn to_mono(interleaved: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    let ch = channels as usize;
    interleaved
        .chunks(ch)
        .map(|frame| frame.iter().sum::<f32>() / ch as f32)
        .collect()
}

/// Resample mono `input` from `from_rate` to [`TARGET_RATE`] via linear
/// interpolation. Returns the input unchanged when it's already at the
/// target rate.
pub fn resample_to_16k(input: &[f32], from_rate: u32) -> Vec<f32> {
    resample(input, from_rate, TARGET_RATE)
}

/// Resample between arbitrary rates. When downsampling, the signal is
/// low-pass filtered to the target Nyquist first (anti-aliasing); linear
/// interpolation then reads samples at the new rate. Upsampling skips the
/// filter — it adds no aliasing, only mild imaging that a bandlimited target
/// like speech tolerates.
pub fn resample(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || input.len() < 2 {
        return input.to_vec();
    }

    // Anti-alias before decimating. Cut a touch below the target Nyquist so
    // the transition band lands in the discarded range rather than folding in.
    let filtered;
    let src: &[f32] = if to_rate < from_rate {
        let cutoff = 0.45 * to_rate as f32;
        filtered = low_pass(input, cutoff, from_rate as f32);
        &filtered
    } else {
        input
    };

    let ratio = to_rate as f64 / from_rate as f64;
    let out_len = ((input.len() as f64) * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    let step = from_rate as f64 / to_rate as f64;
    let last = src.len() - 1;
    for i in 0..out_len {
        let pos = i as f64 * step;
        let idx = pos.floor() as usize;
        if idx >= last {
            out.push(src[last]);
            continue;
        }
        let frac = (pos - idx as f64) as f32;
        out.push(src[idx] * (1.0 - frac) + src[idx + 1] * frac);
    }
    out
}

/// Zero-phase-ish windowed-sinc low-pass FIR. `cutoff_hz` is the −6 dB point;
/// `sample_rate` is the rate of `input`. Edges are zero-padded, so the first
/// and last few samples get mild attenuation — negligible for speech.
fn low_pass(input: &[f32], cutoff_hz: f32, sample_rate: f32) -> Vec<f32> {
    if input.is_empty() {
        return Vec::new();
    }
    let fc = (cutoff_hz / sample_rate).clamp(0.0, 0.5); // normalized cycles/sample
    let half = (LOWPASS_TAPS / 2) as isize;

    // Windowed-sinc kernel (Hann window), normalized to unity DC gain.
    let mut kernel = [0f32; LOWPASS_TAPS];
    let mut sum = 0f32;
    for (i, tap) in kernel.iter_mut().enumerate() {
        let n = i as isize - half;
        let sinc = if n == 0 {
            2.0 * fc
        } else {
            let x = 2.0 * PI * fc * n as f32;
            x.sin() / (PI * n as f32)
        };
        let hann = 0.5 - 0.5 * (2.0 * PI * i as f32 / (LOWPASS_TAPS as f32 - 1.0)).cos();
        *tap = sinc * hann;
        sum += *tap;
    }
    for tap in &mut kernel {
        *tap /= sum;
    }

    // Convolve, keeping the input length (symmetric, zero-padded).
    let n = input.len();
    let mut out = vec![0f32; n];
    for (i, o) in out.iter_mut().enumerate() {
        let mut acc = 0f32;
        for (k, &tap) in kernel.iter().enumerate() {
            let j = i as isize + k as isize - half;
            if j >= 0 && (j as usize) < n {
                acc += input[j as usize] * tap;
            }
        }
        *o = acc;
    }
    out
}

// --- Streaming resampler ---

/// Windowed-sinc low-pass kernel shared by [`low_pass`] and
/// [`StreamingResampler`]. Normalized to unity DC gain.
fn low_pass_kernel(cutoff_hz: f32, sample_rate: f32) -> [f32; LOWPASS_TAPS] {
    let fc = (cutoff_hz / sample_rate).clamp(0.0, 0.5);
    let half = (LOWPASS_TAPS / 2) as isize;
    let mut kernel = [0f32; LOWPASS_TAPS];
    let mut sum = 0f32;
    for (i, tap) in kernel.iter_mut().enumerate() {
        let n = i as isize - half;
        let sinc = if n == 0 {
            2.0 * fc
        } else {
            let x = 2.0 * PI * fc * n as f32;
            x.sin() / (PI * n as f32)
        };
        let hann = 0.5 - 0.5 * (2.0 * PI * i as f32 / (LOWPASS_TAPS as f32 - 1.0)).cos();
        *tap = sinc * hann;
        sum += *tap;
    }
    for tap in &mut kernel {
        *tap /= sum;
    }
    kernel
}

/// Incremental version of [`resample`]: feeds native-rate mic chunks and
/// emits target-rate samples, keeping filter state across chunks.
///
/// Per-chunk [`resample`] calls re-run the low-pass from zero state, so
/// chunk edges get the wrong filter context and the joined output drifts
/// from the one-shot result. This keeps the raw input (pruned past the
/// filter support) plus the interpolation position, delays output samples
/// whose filter support is not fully available yet, and zero-pads the tail
/// on [`flush`](Self::flush) exactly like the one-shot edges. Chunked
/// `push` + `flush` output matches one-shot [`resample`] within 1e-5.
pub struct StreamingResampler {
    from_rate: u32,
    to_rate: u32,
    kernel: Option<[f32; LOWPASS_TAPS]>,
    half: isize,
    /// Raw input samples not yet pruned (absolute index `base + k`).
    input: Vec<f32>,
    /// Absolute input index of `input[0]`.
    base: usize,
    /// Total raw samples ever pushed.
    total_in: usize,
    /// Output samples emitted so far.
    emitted: usize,
    /// True once `flush` ran (tail zero-pads like the one-shot end edge).
    flushed: bool,
}

impl StreamingResampler {
    /// New resampler from `from_rate` to `to_rate`. Same-rate is a
    /// passthrough: `push` echoes the chunk, `flush` is empty.
    pub fn new(from_rate: u32, to_rate: u32) -> Self {
        let kernel = if to_rate < from_rate {
            Some(low_pass_kernel(0.45 * to_rate as f32, from_rate as f32))
        } else {
            None
        };
        Self {
            from_rate,
            to_rate,
            kernel,
            half: (LOWPASS_TAPS / 2) as isize,
            input: Vec::new(),
            base: 0,
            total_in: 0,
            emitted: 0,
            flushed: false,
        }
    }

    fn step(&self) -> f64 {
        f64::from(self.from_rate) / f64::from(self.to_rate)
    }

    fn out_len_total(&self) -> usize {
        ((self.total_in as f64) * f64::from(self.to_rate) / f64::from(self.from_rate)).round()
            as usize
    }

    /// Raw sample at absolute index `a`; zeros outside `[0, total_in)`
    /// once flushed, `None` when the future is not yet known.
    fn raw_at(&self, a: isize) -> Option<f32> {
        if a < 0 {
            return Some(0.0);
        }
        let a = a as usize;
        if a < self.base {
            return None;
        }
        let k = a - self.base;
        if k < self.input.len() {
            return Some(self.input[k]);
        }
        if self.flushed && a < self.total_in {
            return None;
        }
        if self.flushed {
            return Some(0.0);
        }
        None
    }

    /// Filtered value at absolute input index `a` (`None` when its
    /// support needs input that has not arrived yet).
    fn filtered_at(&self, a: usize) -> Option<f32> {
        let Some(kernel) = &self.kernel else {
            return self.raw_at(a as isize);
        };
        let mut acc = 0f32;
        for (k, &tap) in kernel.iter().enumerate() {
            let j = a as isize + k as isize - self.half;
            acc += self.raw_at(j)? * tap;
        }
        Some(acc)
    }

    /// Next output sample, or `None` when its filter support is incomplete.
    /// Mirrors the one-shot interpolation, including the last-sample clamp.
    fn next_sample(&self) -> Option<f32> {
        if self.from_rate == self.to_rate {
            return None;
        }
        let total_out = if self.flushed {
            self.out_len_total()
        } else {
            usize::MAX
        };
        if self.emitted >= total_out {
            return None;
        }
        let pos = self.emitted as f64 * self.step();
        let idx = pos.floor() as usize;
        if self.kernel.is_some() {
            let last = if self.flushed {
                self.total_in.saturating_sub(1)
            } else {
                return match (self.filtered_at(idx), self.filtered_at(idx + 1)) {
                    (Some(a), Some(b)) => {
                        let frac = (pos - idx as f64) as f32;
                        Some(a * (1.0 - frac) + b * frac)
                    }
                    _ => None,
                };
            };
            if idx >= last {
                return self.filtered_at(last);
            }
            match (self.filtered_at(idx), self.filtered_at(idx + 1)) {
                (Some(a), Some(b)) => {
                    let frac = (pos - idx as f64) as f32;
                    Some(a * (1.0 - frac) + b * frac)
                }
                _ => None,
            }
        } else {
            // Upsampling: no filter, interpolate the raw input directly.
            if idx >= self.total_in.saturating_sub(1) {
                if !self.flushed {
                    return None;
                }
                return self.raw_at(self.total_in.saturating_sub(1) as isize);
            }
            match (self.raw_at(idx as isize), self.raw_at(idx as isize + 1)) {
                (Some(a), Some(b)) => {
                    let frac = (pos - idx as f64) as f32;
                    Some(a * (1.0 - frac) + b * frac)
                }
                _ => None,
            }
        }
    }

    /// Drop raw input no future output can reference anymore.
    fn prune(&mut self) {
        if self.input.is_empty() {
            return;
        }
        // Future outputs need filtered/raw at `floor(emitted*step)` and up;
        // the filter reaches `half` behind that.
        let need = (self.emitted as f64 * self.step()).floor() as isize - self.half - 2;
        if need <= self.base as isize {
            return;
        }
        let drop = (need as usize - self.base).min(self.input.len());
        self.input.drain(..drop);
        self.base += drop;
    }

    /// Feed one chunk; returns the newly completed output samples.
    pub fn push(&mut self, chunk: &[f32]) -> Vec<f32> {
        if chunk.is_empty() {
            return Vec::new();
        }
        if self.from_rate == self.to_rate {
            self.total_in += chunk.len();
            self.emitted += chunk.len();
            return chunk.to_vec();
        }
        if self.flushed {
            return Vec::new();
        }
        self.input.extend_from_slice(chunk);
        self.total_in += chunk.len();
        let mut out = Vec::new();
        while let Some(s) = self.next_sample() {
            out.push(s);
            self.emitted += 1;
        }
        self.prune();
        out
    }

    /// End of input: zero-pad the tail like the one-shot end edge and emit
    /// everything up to `round(total_in * to_rate / from_rate)`.
    pub fn flush(&mut self) -> Vec<f32> {
        if self.from_rate == self.to_rate || self.flushed {
            return Vec::new();
        }
        self.flushed = true;
        let mut out = Vec::new();
        while let Some(s) = self.next_sample() {
            out.push(s);
            self.emitted += 1;
        }
        self.prune();
        out
    }
}

/// Load a WAV file as 16 kHz mono f32. Supports integer and float PCM at any
/// rate/channel count; everything is downmixed and resampled.
pub fn load_wav_16k_mono<P: AsRef<Path>>(path: P) -> anyhow::Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max))
                .collect::<Result<_, _>>()?
        }
    };

    let mono = to_mono(&samples, spec.channels);
    Ok(resample_to_16k(&mono, spec.sample_rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_passthrough() {
        let s = vec![0.1, 0.2, 0.3];
        assert_eq!(to_mono(&s, 1), s);
    }

    #[test]
    fn stereo_downmix_averages_channels() {
        // L/R interleaved: (0,1),(2,3) -> 0.5, 2.5
        let s = vec![0.0, 1.0, 2.0, 3.0];
        assert_eq!(to_mono(&s, 2), vec![0.5, 2.5]);
    }

    #[test]
    fn resample_same_rate_is_identity() {
        let s = vec![0.0, 0.5, 1.0];
        assert_eq!(resample(&s, 16_000, 16_000), s);
    }

    #[test]
    fn downsample_halves_length() {
        // 48k -> 16k is a 1/3 ratio, so 300 samples -> 100.
        let s: Vec<f32> = (0..300).map(|i| i as f32).collect();
        let out = resample_to_16k(&s, 48_000);
        assert_eq!(out.len(), 100);
        assert!(out.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn upsample_preserves_endpoints() {
        // Upsampling skips the low-pass, so endpoints pass through untouched.
        let s = vec![0.0, 10.0];
        let out = resample(&s, 8_000, 16_000);
        assert_eq!(out.len(), 4);
        assert!((out[0] - 0.0).abs() < 1e-3);
    }

    fn tone(freq_hz: f32, rate: u32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * PI * freq_hz * i as f32 / rate as f32).sin())
            .collect()
    }

    fn rms(s: &[f32]) -> f32 {
        (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt()
    }

    #[test]
    fn downsample_attenuates_above_target_nyquist() {
        // A 12 kHz tone is above the 8 kHz Nyquist of the 16 kHz target and
        // must be filtered out on the way down, not aliased back into the
        // speech band. A 1 kHz tone passes through. Without anti-aliasing the
        // 12 kHz tone would survive with near-full amplitude.
        let high = resample_to_16k(&tone(12_000.0, 48_000, 4_800), 48_000);
        let low = resample_to_16k(&tone(1_000.0, 48_000, 4_800), 48_000);
        // Ignore filter warm-up/tail at the buffer edges.
        let trim = |s: &[f32]| s[10..s.len() - 10].to_vec();
        assert!(
            rms(&trim(&high)) < 0.2 * rms(&trim(&low)),
            "12kHz not attenuated: high={} low={}",
            rms(&trim(&high)),
            rms(&trim(&low))
        );
    }

    #[test]
    fn streaming_matches_one_shot_downsample() {
        // Speech-ish sum of sines at 48 kHz, odd chunk sizes spanning
        // filter support and interpolation boundaries.
        let n = 48_000;
        let pcm: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / 48_000.0;
                (2.0 * PI * 440.0 * t).sin() * 0.5
                    + (2.0 * PI * 1_200.0 * t).sin() * 0.3
                    + (2.0 * PI * 5_000.0 * t).sin() * 0.1
            })
            .collect();
        let expected = resample_to_16k(&pcm, 48_000);
        for &chunk in &[7usize, 100, 1_000, 1_601, 4_096] {
            let mut s = StreamingResampler::new(48_000, 16_000);
            let mut out = Vec::new();
            for c in pcm.chunks(chunk) {
                out.extend(s.push(c));
            }
            out.extend(s.flush());
            assert_eq!(out.len(), expected.len(), "chunk {chunk}");
            let max = out
                .iter()
                .zip(&expected)
                .map(|(a, b)| (a - b).abs())
                .fold(0f32, f32::max);
            assert!(max < 1e-5, "chunk {chunk}: max diff {max}");
        }
    }

    #[test]
    fn streaming_matches_one_shot_upsample_and_passthrough() {
        let pcm: Vec<f32> = (0..800).map(|i| (i as f32 * 0.11).sin()).collect();
        let expected = resample(&pcm, 8_000, 16_000);
        let mut s = StreamingResampler::new(8_000, 16_000);
        let mut out = Vec::new();
        for c in pcm.chunks(137) {
            out.extend(s.push(c));
        }
        out.extend(s.flush());
        assert_eq!(out.len(), expected.len());
        let max = out
            .iter()
            .zip(&expected)
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        assert!(max < 1e-5, "upsample max diff {max}");
        // Same-rate passthrough echoes chunks; flush is empty.
        let mut s = StreamingResampler::new(16_000, 16_000);
        let mut out = Vec::new();
        for c in pcm.chunks(200) {
            out.extend(s.push(c));
        }
        out.extend(s.flush());
        assert_eq!(out, pcm);
    }
}
