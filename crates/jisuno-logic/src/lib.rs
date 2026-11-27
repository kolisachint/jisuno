//! Platform-independent Jisuno core. No audio, ONNX or network dependencies.

pub mod embedded;
pub mod hold;
pub mod keybindings;
pub mod matcher;
pub mod protocol;
pub mod runner;
pub mod settings;
pub mod vad;
pub mod voice_control;
