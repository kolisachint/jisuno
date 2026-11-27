//! Model worker thread: the ASR helper child plus the streaming
//! utterance protocol (`begin` / `push` / `partial` / `finish`).
//!
//! The model lives in a `jisuno asr-worker` child ([`AsrWorker`]), never in
//! this process: unloading drops the child and the OS reclaims its whole
//! RSS. Loads, finishes and partials block this thread (never the 20 ms
//! tick); `push` never blocks the caller (bounded channel to the child).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use jisuno_asr::setup::Model as AsrModel;
use jisuno_asr::worker::{is_crashed, AsrWorker};
use jisuno_audio::audio;

/// How long `LOAD` may take (cold load is ~1.2 s on the reference Mac).
const LOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// How long one `PARTIAL` re-decode may take (2 min cap ≈ 8 s of decode).
const PARTIAL_TIMEOUT: Duration = Duration::from_secs(30);
/// How long `STATUS` may take (answers from a snapshot).
const STATUS_TIMEOUT: Duration = Duration::from_secs(5);

/// State owned by [`worker_main`]: the helper child (if any), the load
/// generation it installed, and the utterances begun since.
struct HelperEngine {
    worker: Option<AsrWorker>,
    ready_gen: Option<u64>,
    begun: HashSet<u64>,
    /// 16 kHz samples pushed per utterance (drives the finish timeout).
    counts: HashMap<u64, usize>,
}

pub(crate) enum ToWorker {
    Load {
        gen: u64,
        name: String,
    },
    Cancel {
        gen: u64,
    },
    Unload,
    /// Start utterance `stream` (once warm, at mic open).
    Begin {
        stream: u64,
    },
    /// Append 16 kHz mono samples to `stream` (from the audio path).
    Push {
        stream: u64,
        pcm: Vec<f32>,
    },
    /// Decode the audio-so-far of `stream` without ending it.
    PartialReq {
        stream: u64,
    },
    /// End `stream` and decode. `fallback` carries the whole clip for the
    /// cold path (key-up before the load landed): the worker one-shots it
    /// after the load instead.
    Finish {
        stream: u64,
        reply: u64,
        fallback: Option<AudioSource>,
    },
    /// Drop `stream` without decoding (tap / Esc / other-key).
    CancelStream {
        stream: u64,
    },
}

pub(crate) enum AudioSource {
    Mic { rate: u32, pcm: Vec<f32> },
    Wav { path: PathBuf },
}

pub(crate) enum FromWorker {
    Warm {
        gen: u64,
        load_ms: u64,
        child_rss_mb: u64,
    },
    LoadFailed {
        gen: u64,
        err: String,
    },
    Partial {
        stream: u64,
        text: String,
    },
    PartialFailed {
        stream: u64,
    },
    Transcript {
        id: u64,
        text: String,
        decode_ms: u64,
        child_rss_mb: u64,
    },
    NotReady {
        id: u64,
    },
    PendingFailed {
        id: u64,
        err: String,
    },
    /// The helper child is gone. The main thread treats this as unloaded
    /// (HUD error once, next hold restarts).
    Crashed {
        err: String,
    },
}

pub(crate) fn worker_main(rx: mpsc::Receiver<ToWorker>, tx: mpsc::Sender<FromWorker>) {
    let mut engine = HelperEngine {
        worker: None,
        ready_gen: None,
        begun: HashSet::new(),
        counts: HashMap::new(),
    };

    while let Ok(msg) = rx.recv() {
        match msg {
            ToWorker::Load { gen, name } => do_load(&mut engine, &tx, gen, &name),
            // A tap cancels the load its press started: by the time this is
            // processed the load has either failed (nothing to do) or
            // succeeded (drop the child — the owner moved on). The main
            // thread bumps its generation so the stale Warm is ignored. The
            // next hold reloads; that waste is the documented cost of a tap.
            ToWorker::Cancel { gen } => {
                if engine.ready_gen == Some(gen) {
                    engine.worker = None;
                    engine.ready_gen = None;
                    engine.begun.clear();
                    engine.counts.clear();
                }
            }
            ToWorker::Unload => {
                engine.worker = None;
                engine.ready_gen = None;
                engine.begun.clear();
                engine.counts.clear();
            }
            ToWorker::Begin { stream } => {
                if let Some(worker) = engine.worker.as_ref() {
                    match worker.begin(stream) {
                        Ok(()) => {
                            engine.begun.insert(stream);
                            engine.counts.insert(stream, 0);
                        }
                        Err(e) => crashed(&mut engine, &tx, format!("{e:#}")),
                    }
                }
            }
            ToWorker::Push { stream, pcm } => {
                if !engine.begun.contains(&stream) {
                    continue;
                }
                if let Some(worker) = engine.worker.as_ref() {
                    match worker.push(stream, &pcm) {
                        Ok(()) => {
                            *engine.counts.entry(stream).or_insert(0) += pcm.len();
                        }
                        Err(e) => crashed(&mut engine, &tx, format!("{e:#}")),
                    }
                }
            }
            ToWorker::PartialReq { stream } => {
                if !engine.begun.contains(&stream) {
                    let _ = tx.send(FromWorker::PartialFailed { stream });
                    continue;
                }
                if let Some(worker) = engine.worker.as_ref() {
                    match worker.partial(stream, PARTIAL_TIMEOUT) {
                        Ok(text) => {
                            let _ = tx.send(FromWorker::Partial { stream, text });
                        }
                        Err(e) if is_crashed(&e) => {
                            crashed(&mut engine, &tx, format!("{e:#}"));
                        }
                        Err(_) => {
                            let _ = tx.send(FromWorker::PartialFailed { stream });
                        }
                    }
                } else {
                    let _ = tx.send(FromWorker::PartialFailed { stream });
                }
            }
            ToWorker::Finish {
                stream,
                reply,
                fallback,
            } => do_finish(&mut engine, &tx, stream, reply, fallback),
            ToWorker::CancelStream { stream } => {
                engine.begun.remove(&stream);
                engine.counts.remove(&stream);
                if let Some(worker) = engine.worker.as_ref() {
                    if let Err(e) = worker.cancel(stream) {
                        crashed(&mut engine, &tx, format!("{e:#}"));
                    }
                }
            }
        }
    }
}

fn do_load(engine: &mut HelperEngine, tx: &mpsc::Sender<FromWorker>, gen: u64, name: &str) {
    let send = |msg: FromWorker| {
        let _ = tx.send(msg);
    };
    let model = match AsrModel::parse(name) {
        Ok(model) => model,
        Err(e) => {
            send(FromWorker::LoadFailed {
                gen,
                err: format!("{e:#}"),
            });
            return;
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
        return;
    }
    // A stale child (model switch) is dropped first: one model at a time.
    engine.worker = None;
    engine.begun.clear();
    engine.counts.clear();
    engine.ready_gen = None;
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            send(FromWorker::LoadFailed {
                gen,
                err: format!("helper executable: {e:#}"),
            });
            return;
        }
    };
    let worker = match AsrWorker::spawn(&exe, &["asr-worker"]) {
        Ok(worker) => worker,
        Err(e) => {
            send(FromWorker::LoadFailed {
                gen,
                err: format!("spawning asr-worker: {e:#}"),
            });
            return;
        }
    };
    match worker.load(model.id(), LOAD_TIMEOUT) {
        Ok(load_ms) => {
            let rss = child_rss(&worker);
            engine.worker = Some(worker);
            engine.ready_gen = Some(gen);
            send(FromWorker::Warm {
                gen,
                load_ms,
                child_rss_mb: rss,
            });
        }
        Err(e) => {
            send(FromWorker::LoadFailed {
                gen,
                err: format!("{e:#}"),
            });
        }
    }
}

fn do_finish(
    engine: &mut HelperEngine,
    tx: &mpsc::Sender<FromWorker>,
    stream: u64,
    reply: u64,
    fallback: Option<AudioSource>,
) {
    let send = |msg: FromWorker| {
        let _ = tx.send(msg);
    };
    let Some(worker) = engine.worker.as_ref() else {
        send(FromWorker::NotReady { id: reply });
        return;
    };
    if engine.begun.remove(&stream) {
        let samples = engine.counts.remove(&stream).unwrap_or(0);
        let started = Instant::now();
        // Key-up budget: 10 s plus the streamed clip length.
        let timeout = Duration::from_secs(10 + samples.div_ceil(16_000) as u64);
        match worker.finish(stream, timeout) {
            Ok(text) => send(FromWorker::Transcript {
                id: reply,
                text,
                decode_ms: started.elapsed().as_millis() as u64,
                child_rss_mb: child_rss(worker),
            }),
            Err(e) if is_crashed(&e) => crashed(engine, tx, format!("{e:#}")),
            Err(e) => send(FromWorker::PendingFailed {
                id: reply,
                err: format!("{e:#}"),
            }),
        }
        return;
    }
    // Cold path: the utterance never streamed (key-up before the load
    // landed, or the wav-debug run). One-shot the whole clip, queued
    // behind the in-flight load.
    let Some(source) = fallback else {
        send(FromWorker::NotReady { id: reply });
        return;
    };
    let started = Instant::now();
    let pcm: Vec<f32> = match source {
        AudioSource::Mic { rate, pcm } => audio::resample_to_16k(&pcm, rate),
        AudioSource::Wav { path } => match audio::load_wav_16k_mono(&path) {
            Ok(pcm) => pcm,
            Err(e) => {
                send(FromWorker::PendingFailed {
                    id: reply,
                    err: format!("debug wav failed: {e:#}"),
                });
                return;
            }
        },
    };
    let timeout = Duration::from_secs(10 + pcm.len().div_ceil(16_000) as u64);
    match worker.transcribe(&pcm, timeout) {
        Ok(text) => send(FromWorker::Transcript {
            id: reply,
            text,
            decode_ms: started.elapsed().as_millis() as u64,
            child_rss_mb: child_rss(worker),
        }),
        Err(e) if is_crashed(&e) => crashed(engine, tx, format!("{e:#}")),
        Err(e) => send(FromWorker::PendingFailed {
            id: reply,
            err: format!("{e:#}"),
        }),
    }
}

/// The child is gone: drop it, forget the load, and tell the main thread
/// (which resolves any pending transcribe as finished-with-error).
fn crashed(engine: &mut HelperEngine, tx: &mpsc::Sender<FromWorker>, err: String) {
    engine.worker = None;
    engine.ready_gen = None;
    engine.begun.clear();
    engine.counts.clear();
    let _ = tx.send(FromWorker::Crashed { err });
}

/// Best-effort child RSS in MiB, parsed out of the worker `STATUS_REPLY`.
fn child_rss(worker: &AsrWorker) -> u64 {
    let Ok(json) = worker.status(STATUS_TIMEOUT) else {
        return 0;
    };
    json.split_once("\"rss_mb\":")
        .and_then(|(_, rest)| {
            rest.chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<u64>()
                .ok()
        })
        .unwrap_or(0)
}
