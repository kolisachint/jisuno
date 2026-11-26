//! Transcription behind a single trait, so the mic/VAD pipeline doesn't
//! depend on the ONNX details.

use crate::setup::Model;

pub mod parakeet;

/// A speech-to-text backend.
pub trait Transcriber {
    /// Transcribe 16 kHz mono f32 PCM. `on_segment` is called for each chunk
    /// of recognized text (typically a word) as it is decoded, so callers can
    /// stream output rather than waiting for the whole utterance.
    fn transcribe(&mut self, pcm: &[f32], on_segment: &mut dyn FnMut(&str)) -> anyhow::Result<()>;
}

/// Load the Parakeet transcriber for `model` from its install directory.
pub fn load(model: Model) -> anyhow::Result<Box<dyn Transcriber>> {
    let dir = model.dir()?;
    Ok(Box::new(parakeet::ParakeetTranscriber::load(&dir)?))
}
