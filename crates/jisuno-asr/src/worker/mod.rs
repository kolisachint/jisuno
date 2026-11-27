//! ASR helper process: run the model in a child so unloading it returns
//! all its memory (the OS reclaims everything on child exit; in-process
//! unload keeps ~650 MiB in ONNX Runtime / malloc).
//!
//! Wire format is binary framed little-endian over stdin/stdout, stderr
//! for logs. See [`frame`] for the codec, [`server`] for the child side,
//! [`client`] for the parent side.

pub mod client;
pub mod frame;
pub mod server;

pub use client::AsrWorker;
pub use server::run_worker_stdio;
