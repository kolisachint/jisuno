//! Start, stop and cancel cues (`app-ui.md` §3.2, `sounds` in `settings.json`).
//!
//! Each tone is a short sine with a raised-cosine fade in and out, built in
//! memory on demand (no audio files). [`play`] runs the output stream on its
//! own thread, so the caller (the 20 ms tick) never waits on the audio device.

use std::f32::consts::PI;

/// Tone length (`app-ui.md` §3.2: short, under 120 ms).
pub const TONE_MS: u64 = 60;
/// Peak amplitude: about -18 dBFS (`20 * log10(0.125)` = -18.1).
const PEAK: f32 = 0.125;
const FADE_IN_MS: f32 = 5.0;
const FADE_OUT_MS: f32 = 20.0;

/// One cue. Frequencies are the v2/v3 prototype values (`app-ui.md` §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    StartDictate,
    StartCommand,
    StopDictate,
    StopCommand,
    Cancel,
}

impl Tone {
    pub fn freq_hz(self) -> f32 {
        match self {
            Self::StartDictate => 880.0,
            Self::StartCommand => 660.0,
            Self::StopDictate => 1320.0,
            Self::StopCommand => 990.0,
            Self::Cancel => 300.0,
        }
    }
}

/// Raised-cosine ramp from 0 at `t = 0` to 1 at `t = 1`.
fn ramp(t: f32) -> f32 {
    0.5 - 0.5 * (PI * t).cos()
}

/// Mono samples of one tone at `sample_rate`. The first and last samples are
/// exactly zero, so the tone starts and stops without a click.
pub fn synthesize(freq_hz: f32, sample_rate: u32) -> Vec<f32> {
    let n = (u64::from(sample_rate) * TONE_MS / 1000) as usize;
    let fade_in = ((sample_rate as f32 * FADE_IN_MS / 1000.0) as usize).max(1);
    let fade_out = ((sample_rate as f32 * FADE_OUT_MS / 1000.0) as usize).max(1);
    let omega = 2.0 * PI * freq_hz / sample_rate as f32;
    (0..n)
        .map(|i| {
            let rise = if i < fade_in {
                ramp(i as f32 / fade_in as f32)
            } else {
                1.0
            };
            let left = n - 1 - i;
            let fall = if left < fade_out {
                ramp(left as f32 / fade_out as f32)
            } else {
                1.0
            };
            PEAK * rise.min(fall) * (omega * i as f32).sin()
        })
        .collect()
}

/// Play one cue on the default output device, on a background thread.
/// Errors are logged to stderr; they never reach the caller.
#[cfg(feature = "mic")]
pub fn play(tone: Tone) {
    let spawned = std::thread::Builder::new()
        .name("jisuno-tone".to_string())
        .spawn(move || {
            if let Err(e) = play_now(tone) {
                eprintln!("[tone] {e:#}");
            }
        });
    if let Err(e) = spawned {
        eprintln!("[tone] thread spawn failed: {e}");
    }
}

#[cfg(feature = "mic")]
fn play_now(tone: Tone) -> anyhow::Result<()> {
    use anyhow::Context;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::SampleFormat;
    use std::time::Duration;

    let device = cpal::default_host()
        .default_output_device()
        .context("no default output device")?;
    let config = device
        .default_output_config()
        .context("failed to read default output config")?;
    let samples = synthesize(tone.freq_hz(), config.sample_rate().0);
    let channels = usize::from(config.channels());
    let format = config.sample_format();
    let stream_config: cpal::StreamConfig = config.into();

    let stream = match format {
        SampleFormat::F32 => output::<f32>(&device, &stream_config, channels, samples)?,
        SampleFormat::I16 => output::<i16>(&device, &stream_config, channels, samples)?,
        SampleFormat::U16 => output::<u16>(&device, &stream_config, channels, samples)?,
        other => anyhow::bail!("unsupported output sample format: {other:?}"),
    };
    stream.play().context("failed to start output stream")?;
    // The stream plays the buffer once and then writes silence; keep it
    // alive for the tone plus a small tail, then release the device.
    std::thread::sleep(Duration::from_millis(TONE_MS + 20));
    drop(stream);
    Ok(())
}

/// Output stream that plays `samples` once on every channel, then silence.
#[cfg(feature = "mic")]
fn output<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    samples: Vec<f32>,
) -> anyhow::Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    use anyhow::Context;
    use cpal::traits::DeviceTrait;

    let mut pos = 0usize;
    device
        .build_output_stream(
            config,
            move |out: &mut [T], _| {
                for frame in out.chunks_mut(channels) {
                    let v = samples.get(pos).copied().unwrap_or(0.0);
                    pos = (pos + 1).min(samples.len());
                    for s in frame.iter_mut() {
                        *s = T::from_sample(v);
                    }
                }
            },
            |e| eprintln!("[tone] stream error: {e}"),
            None,
        )
        .context("failed to build output stream")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0_f32, |m, s| m.max(s.abs()))
    }

    #[test]
    fn length_is_sixty_ms_and_under_the_limit() {
        assert_eq!(synthesize(880.0, 48_000).len(), 2_880);
        assert_eq!(synthesize(880.0, 44_100).len(), 2_646);
    }

    #[test]
    fn peak_is_about_minus_18_dbfs() {
        for (tone, rate) in [
            (Tone::StartDictate, 48_000),
            (Tone::Cancel, 16_000),
            (Tone::StopDictate, 44_100),
        ] {
            let p = peak(&synthesize(tone.freq_hz(), rate));
            assert!(p <= PEAK + 1e-6, "{tone:?} peak {p} above the target");
            assert!(p > 0.11, "{tone:?} peak {p} too quiet");
        }
    }

    #[test]
    fn starts_and_stops_at_zero_without_clicks() {
        for tone in [
            Tone::StartDictate,
            Tone::StartCommand,
            Tone::StopDictate,
            Tone::StopCommand,
            Tone::Cancel,
        ] {
            let rate = 48_000;
            let s = synthesize(tone.freq_hz(), rate);
            assert_eq!(s.first().copied(), Some(0.0));
            assert_eq!(s.last().copied(), Some(0.0));
            // Largest sample-to-sample step: the sine's own slope plus the
            // fade's slope, with margin. A hard edge would exceed this.
            let omega = 2.0 * PI * tone.freq_hz() / rate as f32;
            let fade = (rate as f32 * FADE_IN_MS / 1000.0).max(1.0);
            let bound = PEAK * (omega + PI / fade) * 1.5;
            let step = s
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0_f32, f32::max);
            assert!(step <= bound, "{tone:?} step {step} > {bound}");
        }
    }
}
