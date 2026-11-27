//! `asr_worker` example: the helper-process entry point.
//!
//! The app spawns this (hidden `jisuno asr-worker` subcommand will call
//! [`jisuno_asr::worker::run_worker_stdio`]). With `--fake` it serves the
//! fake backend (text = sample count) so lifecycle tests need no model:
//! `JISUNO_FAKE_WORKER=1` selects it inside the worker.

fn main() -> anyhow::Result<()> {
    if std::env::args().any(|a| a == "--fake") {
        // SAFETY: single-threaded at startup; no other thread exists yet.
        unsafe { std::env::set_var("JISUNO_FAKE_WORKER", "1") };
    }
    jisuno_asr::worker::run_worker_stdio()
}
