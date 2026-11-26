//! Transcription behind a single trait, so the mic/VAD pipeline doesn't
//! depend on the ONNX details.

use crate::setup::Model;

pub mod parakeet;

pub use parakeet::{ParakeetStreamingSession, StreamConfig};

/// A speech-to-text backend.
pub trait Transcriber {
    /// Transcribe 16 kHz mono f32 PCM. `on_segment` is called for each chunk
    /// of recognized text (typically a word) as it is decoded, so callers can
    /// stream output rather than waiting for the whole utterance.
    fn transcribe(&mut self, pcm: &[f32], on_segment: &mut dyn FnMut(&str)) -> anyhow::Result<()>;
}

/// Incremental transcription: audio is fed in while the hold key is down,
/// pause-bounded segments are decoded on a background thread, and key-up
/// (`finish`) only decodes the tail since the last pause. See
/// `ParakeetStreamingSession`.
pub trait StreamingTranscriber {
    /// Append 16 kHz mono f32 PCM. Buffers the audio, runs the energy gate,
    /// and queues newly cut segments for background decode. Cheap: no ONNX
    /// work happens here.
    fn push(&mut self, pcm_16k: &[f32]) -> anyhow::Result<()>;
    /// Text of fully decoded segments so far, or `None` when nothing has
    /// been recognized yet.
    fn partial(&self) -> Option<String>;
    /// Wait for queued segments, decode the tail since the last cut, and
    /// return the full transcript. This is the key-up fast path when speech
    /// has pauses. May be called again after more `push` calls; it does
    /// not reset the session.
    fn finish(&mut self) -> anyhow::Result<String>;
    /// Drop all buffered audio and segment texts. Sessions (the loaded
    /// model) are kept.
    fn reset(&mut self);
}

/// Load the Parakeet transcriber for `model` from its install directory.
pub fn load(model: Model) -> anyhow::Result<Box<dyn Transcriber>> {
    let dir = model.dir()?;
    Ok(Box::new(parakeet::ParakeetTranscriber::load(&dir)?))
}

/// Load a [`StreamingTranscriber`] with the default [`StreamConfig`].
pub fn load_streaming(model: Model) -> anyhow::Result<Box<dyn StreamingTranscriber>> {
    load_streaming_with_config(model, StreamConfig::default())
}

/// Load a [`StreamingTranscriber`] with an explicit pause-split config.
pub fn load_streaming_with_config(
    model: Model,
    config: StreamConfig,
) -> anyhow::Result<Box<dyn StreamingTranscriber>> {
    let dir = model.dir()?;
    let t = ParakeetStreamingSession::load(&dir, config)?;
    Ok(Box::new(t))
}
