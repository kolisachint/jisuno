//! Audio helpers (downmix, 16 kHz resample, WAV loading) and, behind the
//! `mic` feature, `cpal` microphone capture.

pub mod audio;
#[cfg(feature = "mic")]
pub mod mic;
