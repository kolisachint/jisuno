//! Model worker thread: full-clip decode behind `FullClipDecode`.
//!
//! Split from `app.rs` with no behavior change (step 1).

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

use anyhow::Result;
use jisuno_asr::setup::Model as AsrModel;
use jisuno_asr::transcribe::Transcriber;
use jisuno_audio::audio;

/// Full-clip decode behind one method, so the streaming path being built
/// separately can replace this without touching the app wiring.
pub(super) trait FullClipDecode {
    fn ensure_loaded(&mut self, model: AsrModel) -> Result<()>;
    fn decode_clip(&mut self, pcm_16k: &[f32]) -> Result<String>;
    fn unload(&mut self);
}

struct ParakeetEngine {
    inner: Option<Box<dyn Transcriber>>,
}

impl FullClipDecode for ParakeetEngine {
    fn ensure_loaded(&mut self, model: AsrModel) -> Result<()> {
        if self.inner.is_none() {
            self.inner = Some(jisuno_asr::transcribe::load(model)?);
        }
        Ok(())
    }

    fn decode_clip(&mut self, pcm_16k: &[f32]) -> Result<String> {
        let engine = self
            .inner
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("decode with no model loaded"))?;
        let mut text = String::new();
        engine.transcribe(pcm_16k, &mut |seg: &str| {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(seg);
        })?;
        Ok(text)
    }

    fn unload(&mut self) {
        self.inner = None;
    }
}

pub(crate) enum ToWorker {
    Load { gen: u64, name: String },
    Cancel { gen: u64 },
    Unload,
    Transcribe { id: u64, source: AudioSource },
}

pub(crate) enum AudioSource {
    Mic { rate: u32, pcm: Vec<f32> },
    Wav { path: PathBuf },
}

pub(crate) enum FromWorker {
    Warm {
        gen: u64,
    },
    LoadFailed {
        gen: u64,
        err: String,
    },
    Transcript {
        id: u64,
        text: String,
        decode_ms: u64,
    },
    NotReady {
        id: u64,
    },
    PendingFailed {
        id: u64,
        err: String,
    },
}

pub(crate) fn worker_main(rx: mpsc::Receiver<ToWorker>, tx: mpsc::Sender<FromWorker>) {
    let mut engine = ParakeetEngine { inner: None };
    // The load this generation installed, if any. Messages are processed in
    // order and loads block, so a Transcribe sent while the main thread
    // believes the model is loading simply queues behind the Load and
    // decodes as soon as it lands (only the remainder adds latency).
    let mut ready_gen: Option<u64> = None;
    let send = |msg: FromWorker| {
        let _ = tx.send(msg);
    };

    while let Ok(msg) = rx.recv() {
        match msg {
            ToWorker::Load { gen, name } => {
                let model = match AsrModel::parse(&name) {
                    Ok(model) => model,
                    Err(e) => {
                        send(FromWorker::LoadFailed {
                            gen,
                            err: format!("{e:#}"),
                        });
                        continue;
                    }
                };
                if !model.is_ready() {
                    send(FromWorker::LoadFailed {
                        gen,
                        err: format!(
                            "{} is not downloaded — run: jisuno setup --model {}",
                            model.id(),
                            model.id()
                        ),
                    });
                    continue;
                }
                match engine.ensure_loaded(model) {
                    Ok(()) => {
                        ready_gen = Some(gen);
                        send(FromWorker::Warm { gen });
                    }
                    Err(e) => {
                        ready_gen = None;
                        send(FromWorker::LoadFailed {
                            gen,
                            err: format!("{e:#}"),
                        });
                    }
                }
            }
            // A tap cancels the load its press started: by the time this is
            // processed the load has either failed (nothing to do) or
            // succeeded (unload it — the owner moved on). The main thread
            // bumps its generation so the stale Warm is ignored. The next
            // hold reloads; that waste is the documented cost of a tap.
            ToWorker::Cancel { gen } => {
                if ready_gen == Some(gen) {
                    engine.unload();
                    ready_gen = None;
                }
            }
            ToWorker::Unload => {
                engine.unload();
                ready_gen = None;
            }
            ToWorker::Transcribe { id, source } => {
                if engine.inner.is_some() {
                    decode_and_send(&mut engine, &send, id, source);
                } else {
                    send(FromWorker::NotReady { id });
                }
            }
        }
    }
}

fn decode_and_send(
    engine: &mut ParakeetEngine,
    send: &impl Fn(FromWorker),
    id: u64,
    source: AudioSource,
) {
    let started = Instant::now();
    let pcm: Vec<f32> = match source {
        AudioSource::Mic { rate, pcm } => audio::resample_to_16k(&pcm, rate),
        AudioSource::Wav { path } => match audio::load_wav_16k_mono(&path) {
            Ok(pcm) => pcm,
            Err(e) => {
                send(FromWorker::PendingFailed {
                    id,
                    err: format!("debug wav failed: {e:#}"),
                });
                return;
            }
        },
    };
    match engine.decode_clip(&pcm) {
        Ok(text) => send(FromWorker::Transcript {
            id,
            text,
            decode_ms: started.elapsed().as_millis() as u64,
        }),
        Err(e) => send(FromWorker::PendingFailed {
            id,
            err: format!("{e:#}"),
        }),
    }
}
