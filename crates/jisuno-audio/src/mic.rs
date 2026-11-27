//! Microphone capture via `cpal`.
//!
//! We stream **native-rate mono** chunks over a channel rather than resampling
//! inside the realtime audio callback. VAD runs on these chunks (RMS is
//! sample-rate independent), and the full buffer is resampled to 16 kHz once,
//! right before transcription — this avoids the phase discontinuities you'd
//! get from resampling each callback buffer independently.

use std::sync::mpsc::{channel, Receiver};

use anyhow::{anyhow, Context};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};

use crate::audio::to_mono;

/// A live capture session. Hold onto it to keep the stream running; drop it
/// (or call [`Capture::stop`]) to release the device.
pub struct Capture {
    /// Native-rate mono chunks from the input device.
    pub chunks: Receiver<Vec<f32>>,
    /// The device's sample rate; resample the captured buffer from this.
    pub input_rate: u32,
    stream: Stream,
}

impl Capture {
    /// Explicitly stop and release the input stream.
    pub fn stop(self) {
        drop(self.stream);
    }
}

/// What `settings.json` `input_device` resolves to on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputChoice<'a> {
    /// `system`: the default input device.
    System,
    /// A connected device with exactly this name.
    Named(&'a str),
    /// A name that is not connected: the caller falls back to the system
    /// input and notes it (`app-ui.md` H17).
    Missing,
}

/// Pure selection for `input_device` over the connected device names.
pub fn choose_input<'a>(wanted: &str, available: &'a [String]) -> InputChoice<'a> {
    if wanted == "system" {
        return InputChoice::System;
    }
    match available.iter().find(|name| name.as_str() == wanted) {
        Some(name) => InputChoice::Named(name.as_str()),
        None => InputChoice::Missing,
    }
}

/// Open the default input device and begin streaming mono chunks.
pub fn start() -> anyhow::Result<Capture> {
    let device = cpal::default_host()
        .default_input_device()
        .context("no default input device — is a microphone connected?")?;
    open(device)
}

/// Open the connected input device named `name` (exact match).
pub fn start_named(name: &str) -> anyhow::Result<Capture> {
    let device = cpal::default_host()
        .input_devices()
        .context("failed to list input devices")?
        .find(|d| d.name().is_ok_and(|n| n == name))
        .with_context(|| format!("no input device named \"{name}\""))?;
    open(device)
}

fn open(device: cpal::Device) -> anyhow::Result<Capture> {
    let config = device
        .default_input_config()
        .context("failed to read default input config")?;
    let input_rate = config.sample_rate().0;
    let channels = config.channels();
    let sample_format = config.sample_format();
    let stream_config: cpal::StreamConfig = config.into();

    let (tx, rx) = channel::<Vec<f32>>();
    let err_fn = |e| eprintln!("[mic] stream error: {e}");

    // Each branch converts the device's native sample type to f32, downmixes
    // to mono, and forwards the chunk. Send errors mean the receiver hung up,
    // which simply ends capture — so they're ignored.
    let stream = match sample_format {
        SampleFormat::F32 => device.build_input_stream(
            &stream_config,
            move |data: &[f32], _| {
                let _ = tx.send(to_mono(data, channels));
            },
            err_fn,
            None,
        ),
        SampleFormat::I16 => device.build_input_stream(
            &stream_config,
            move |data: &[i16], _| {
                let f: Vec<f32> = data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                let _ = tx.send(to_mono(&f, channels));
            },
            err_fn,
            None,
        ),
        SampleFormat::U16 => device.build_input_stream(
            &stream_config,
            move |data: &[u16], _| {
                let f: Vec<f32> = data
                    .iter()
                    .map(|&s| (s as f32 / u16::MAX as f32) * 2.0 - 1.0)
                    .collect();
                let _ = tx.send(to_mono(&f, channels));
            },
            err_fn,
            None,
        ),
        other => return Err(anyhow!("unsupported sample format: {other:?}")),
    }
    .context("failed to build input stream")?;

    stream.play().context("failed to start input stream")?;

    Ok(Capture {
        chunks: rx,
        input_rate,
        stream,
    })
}

#[cfg(test)]
mod tests {
    use super::{choose_input, InputChoice};

    fn devices() -> Vec<String> {
        vec![
            "MacBook Air Microphone".to_string(),
            "Scarlett 2i2".to_string(),
        ]
    }

    #[test]
    fn system_is_the_default_input_whatever_is_connected() {
        assert_eq!(choose_input("system", &devices()), InputChoice::System);
        assert_eq!(choose_input("system", &[]), InputChoice::System);
    }

    #[test]
    fn a_connected_name_opens_that_device() {
        assert_eq!(
            choose_input("Scarlett 2i2", &devices()),
            InputChoice::Named("Scarlett 2i2")
        );
    }

    #[test]
    fn a_name_not_connected_is_missing() {
        assert_eq!(choose_input("Scarlett 2i2", &[]), InputChoice::Missing);
        assert_eq!(choose_input("USB Mic", &devices()), InputChoice::Missing);
    }

    #[test]
    fn names_match_exactly() {
        assert_eq!(
            choose_input("scarlett 2i2", &devices()),
            InputChoice::Missing
        );
    }
}
