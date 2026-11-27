//! Model lifecycle: load/cancel/unload, idle unload, worker drain.
//!
//! Split from `app.rs` with no behavior change (step 1).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::App;
use jisuno_logic::hold::Input;
use jisuno_logic::settings::{self, Settings};
use jisuno_ui::hud_model::{tail_text, HudState as UiHud, PARTIAL_TAIL_CHARS};

use super::state::{text_summary, AppState};
use super::worker::{worker_main, FromWorker, ToWorker};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelState {
    Unloaded,
    Loading { gen: u64 },
    Warm,
    Failed,
}

/// A model download running on its own thread, reporting over `download_tx`.
/// One at a time: starting another cancels the running one.
pub(crate) struct ActiveDownload {
    pub(crate) model_id: String,
    pub(crate) percent: u32,
    pub(crate) cancel: Arc<AtomicBool>,
}

/// Progress/completion reports from the download thread.
pub(crate) enum DownloadEvent {
    Progress { model_id: String, percent: u32 },
    Done { model_id: String },
    Failed { model_id: String, err: String },
}

pub(crate) fn model_id(settings: &Settings) -> String {
    match settings.model {
        settings::Model::ParakeetV3Int8 => "parakeet-tdt-0.6b-v3-int8".to_string(),
        settings::Model::ParakeetV2Int8 => "parakeet-tdt-0.6b-v2-int8".to_string(),
    }
}

// --- Model lifecycle ---

impl AppState {
    // --- Model lifecycle ---

    pub(crate) fn start_load(&mut self, cx: &mut App, why: &str) {
        if self.model_state == ModelState::Warm {
            self.log(&format!("warm ({why}): already warm"));
            return;
        }
        if matches!(self.model_state, ModelState::Loading { gen } if gen == self.model_gen) {
            self.log(&format!("warm ({why}): already loading"));
            return;
        }
        if !self.model_files_present() {
            self.log(&format!("warm ({why}): model not downloaded"));
            self.show_ephemeral(cx, UiHud::ModelMissing, "model missing");
            return;
        }
        self.model_gen += 1;
        let gen = self.model_gen;
        self.model_state = ModelState::Loading { gen };
        self.machine
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .handle(Input::LoadStarted, self.now_ms());
        let _ = self.to_worker.send(ToWorker::Load {
            gen,
            name: self.model_name.clone(),
        });
        self.log(&format!("model load started ({why})"));
    }

    pub(crate) fn cancel_load(&mut self) {
        if let ModelState::Loading { gen } = self.model_state {
            // Settle locally first so the in-flight Warm is ignored; the
            // worker unloads whatever the press started.
            self.model_gen += 1;
            self.model_state = ModelState::Unloaded;
            let _ = self.to_worker.send(ToWorker::Cancel { gen });
            self.log("model load cancelled (tap)");
        }
    }

    /// Start (or resume) downloading `model_id` on a thread; progress and
    /// completion arrive as [`DownloadEvent`]s. A running download for
    /// another model is cancelled first.
    pub(crate) fn start_download(&mut self, model_id: String) {
        if let Some(active) = &self.download {
            if active.model_id == model_id {
                // A second click cancels the running download.
                self.cancel_download("toggle");
                return;
            }
            active.cancel.store(true, Ordering::SeqCst);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let tx = self.download_tx.clone();
        let cancel_w = Arc::clone(&cancel);
        let id = model_id.clone();
        self.download = Some(ActiveDownload {
            model_id: model_id.clone(),
            percent: 0,
            cancel,
        });
        self.log(&format!("download {model_id}: started"));
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<()> {
                let model = jisuno_asr::setup::Model::parse(&id)?;
                if model.is_ready() {
                    return Ok(());
                }
                let tls = jisuno_asr::tls::TlsOptions::default();
                jisuno_asr::setup::download_model(
                    &model,
                    &tls,
                    &cancel_w,
                    &mut |p: jisuno_asr::download::Progress| {
                        let percent = p
                            .overall_done
                            .saturating_mul(100)
                            .checked_div(p.overall_total)
                            .map(|q| q.min(100) as u32)
                            .unwrap_or(0);
                        let _ = tx.send(DownloadEvent::Progress {
                            model_id: id.clone(),
                            percent,
                        });
                    },
                )?;
                if cancel_w.load(Ordering::SeqCst) {
                    anyhow::bail!("cancelled");
                }
                model.verify()?;
                Ok(())
            })();
            match result {
                Ok(()) => {
                    let _ = tx.send(DownloadEvent::Done { model_id: id });
                }
                Err(e) => {
                    let _ = tx.send(DownloadEvent::Failed {
                        model_id: id,
                        err: format!("{e:#}"),
                    });
                }
            }
        });
    }

    /// Cancel the running download, if any.
    pub(crate) fn cancel_download(&mut self, why: &str) {
        if let Some(active) = self.download.take() {
            active.cancel.store(true, Ordering::SeqCst);
            self.log(&format!("download {} cancelled ({why})", active.model_id));
        }
    }

    /// Drain download progress/completion into state (the tick calls this).
    pub(crate) fn drain_download(&mut self) {
        let mut events = Vec::new();
        while let Ok(event) = self.download_rx.try_recv() {
            events.push(event);
        }
        for event in events {
            match event {
                DownloadEvent::Progress { model_id, percent } => {
                    if let Some(active) = &mut self.download {
                        if active.model_id == model_id {
                            active.percent = percent;
                        }
                    }
                }
                DownloadEvent::Done { model_id } => {
                    self.download = None;
                    self.model_present = self.model_files_present();
                    self.log(&format!("download {model_id}: finished"));
                }
                DownloadEvent::Failed { model_id, err } => {
                    self.download = None;
                    self.model_present = self.model_files_present();
                    self.log(&format!("download {model_id}: failed: {err}"));
                }
            }
        }
    }

    /// Unload is only safe with the mic closed and no transcribe pending:
    /// unloading mid-hold drops the utterance being dictated. The socket
    /// handler (socket.rs) maps this to a `busy` reply via the tick-
    /// refreshed snapshot; menu/key/command paths just refuse and log.
    pub(crate) fn can_unload(&self) -> bool {
        !self.mic_open && self.pending.is_none()
    }

    pub(crate) fn unload_model(&mut self, why: &str) {
        if !self.can_unload() {
            self.log(&format!(
                "unload ({why}) refused: busy (mic open or transcribe pending)"
            ));
            return;
        }
        if self.model_state == ModelState::Warm {
            let _ = self.to_worker.send(ToWorker::Unload);
            self.model_state = ModelState::Unloaded;
            self.machine
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .set_model_warm(false);
            // The helper child exits; the OS reclaims its whole RSS.
            self.child_rss_mb = None;
            self.set_socket_rss(None);
            self.log(&format!("model unloaded ({why})"));
        } else {
            self.log(&format!("unload ({why}): nothing loaded"));
        }
    }

    fn set_socket_rss(&self, rss_mb: Option<u64>) {
        if let Ok(mut snapshot) = self.socket_snapshot.lock() {
            snapshot.rss_mb = rss_mb;
        }
    }

    pub(crate) fn poll_idle_unload(&mut self) {
        let mins = self.settings.unload_after_idle_min;
        if mins == 0 || !self.can_unload() || self.model_state != ModelState::Warm {
            return;
        }
        if self.last_active.elapsed() >= Duration::from_secs(u64::from(mins) * 60) {
            self.unload_model("idle");
        }
    }

    /// A hold that started cold buffered audio while the load ran: begin
    /// the stream now and flush what was captured so far. If key-up already
    /// beat the load (`pending` set), the queued Finish carries the full
    /// clip instead — drop the reserved stream so it never leaks.
    fn flush_cold_stream(&mut self) {
        let Some(stream) = self.stream else {
            return;
        };
        if !self.mic_open || self.stream_begun {
            return;
        }
        if self.pending.is_some() {
            self.clear_stream();
            return;
        }
        if self.to_worker.send(ToWorker::Begin { stream }).is_err() {
            self.stream = None;
            self.log("model worker gone — stream dropped");
            return;
        }
        // The buffer is already 16 kHz (drained incrementally per chunk),
        // debug WAV included: send it as-is instead of resampling the
        // whole clip on the tick. The resampler keeps its state so the
        // following chunks continue with the right filter context.
        let pcm16 = std::mem::take(&mut self.buffer);
        if self
            .to_worker
            .send(ToWorker::Push { stream, pcm: pcm16 })
            .is_err()
        {
            self.stream = None;
            self.log("model worker gone — stream dropped");
            return;
        }
        self.stream_begun = true;
        self.last_partial_poll = Instant::now();
        self.log(&format!("stream {stream} begun (warm flush)"));
    }

    /// Spawn a fresh worker thread on new channels, replacing the dead one.
    pub(crate) fn respawn_worker(&mut self) {
        let (to_worker, worker_rx) = mpsc::channel();
        let (worker_tx, from_worker) = mpsc::channel();
        self.to_worker = to_worker;
        self.from_worker = from_worker;
        std::thread::spawn(move || worker_main(worker_rx, worker_tx));
        self.log("model worker respawned");
    }

    /// The worker thread is gone (a `to_worker` send failed): respawn it,
    /// forget the load, and start it again. The caller re-drives its
    /// utterance from fallback audio, which the cold path decodes behind
    /// the fresh load — dictation survives the restart.
    pub(crate) fn recover_worker(&mut self, cx: &mut App, why: &str) {
        self.respawn_worker();
        self.model_state = ModelState::Unloaded;
        self.machine
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_model_warm(false);
        self.child_rss_mb = None;
        self.start_load(cx, why);
    }

    pub(crate) fn clear_stream(&mut self) {
        self.stream = None;
        self.stream_begun = false;
        self.partial_in_flight = false;
        self.partial_text = None;
    }

    /// The helper child is gone: treat it as unloaded, show the HUD error
    /// once, and let the next hold restart (it spawns a fresh child).
    fn on_worker_crashed(&mut self, cx: &mut App, err: &str) {
        self.clear_stream();
        self.child_rss_mb = None;
        self.set_socket_rss(None);
        if self.model_state != ModelState::Unloaded {
            self.log(&format!("asr worker crashed: {err}"));
            self.model_state = ModelState::Unloaded;
            self.machine
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .set_model_warm(false);
        }
        // Invalidate any Warm still queued behind the crash.
        self.model_gen = self.model_gen.wrapping_add(1);
        if !self.worker_crash_shown {
            self.worker_crash_shown = true;
            self.show_ephemeral(
                cx,
                UiHud::ConfigError {
                    location: "asr-worker".to_string(),
                    message: format!("speech helper exited ({err}) · restarts on next hold"),
                },
                "worker crashed",
            );
        }
        if let Some(pending) = self.pending.take() {
            self.input(
                cx,
                Input::Finished {
                    hud: None,
                    id: pending.id,
                },
                "finished (worker crashed)",
            );
        }
    }

    pub(crate) fn drain_worker(&mut self, cx: &mut App) {
        while let Ok(msg) = self.from_worker.try_recv() {
            match msg {
                FromWorker::Warm {
                    gen,
                    load_ms,
                    child_rss_mb,
                } => {
                    if gen != self.model_gen {
                        continue;
                    }
                    self.model_state = ModelState::Warm;
                    self.last_active = Instant::now();
                    self.machine
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .handle(Input::ModelWarm, self.now_ms());
                    self.child_rss_mb = Some(child_rss_mb);
                    self.set_socket_rss(Some(child_rss_mb));
                    self.worker_crash_shown = false;
                    self.log(&format!(
                        "model warm (load {load_ms} ms, child {child_rss_mb} MB)"
                    ));
                    self.flush_cold_stream();
                    if !self.mic_open && self.pending.is_none() {
                        self.show_ephemeral(cx, UiHud::ModelReady, "model ready");
                    }
                }
                FromWorker::LoadFailed { gen, err } => {
                    if gen != self.model_gen {
                        continue;
                    }
                    self.model_state = ModelState::Failed;
                    self.machine
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .handle(Input::LoadFailed, self.now_ms());
                    self.log(&format!("model load failed: {err}"));
                }
                FromWorker::Partial { stream, text } => {
                    self.partial_in_flight = false;
                    if self.settings.partials && self.stream == Some(stream) && self.mic_open {
                        let tail = tail_text(text.trim(), PARTIAL_TAIL_CHARS).to_string();
                        if !tail.is_empty() {
                            self.partial_text = Some(tail.clone());
                            if let Some(listening) = &mut self.listening {
                                listening.partial = Some(tail.clone());
                            }
                            self.log(&format!("partial: {}", text_summary(&tail)));
                            self.refresh_hud_now(cx);
                        }
                    }
                }
                FromWorker::PartialFailed { stream } => {
                    if self.stream == Some(stream) {
                        self.partial_in_flight = false;
                    }
                }
                FromWorker::Transcript {
                    id,
                    text,
                    decode_ms,
                    child_rss_mb,
                } => {
                    self.on_transcript(cx, id, text, decode_ms, child_rss_mb);
                    self.set_socket_rss(self.child_rss_mb);
                }
                FromWorker::NotReady { id } => {
                    self.log(&format!("worker not ready for {id}"));
                    self.clear_stream();
                    if self.pending.take().is_some() {
                        self.input(
                            cx,
                            Input::Finished { hud: None, id },
                            "finished (not ready)",
                        );
                        self.show_ephemeral(cx, UiHud::ModelMissing, "model not ready");
                    }
                }
                FromWorker::PendingFailed { id, err } => {
                    self.log(&format!("transcribe {id} failed: {err}"));
                    self.clear_stream();
                    if self.pending.take().is_some() {
                        self.input(
                            cx,
                            Input::Finished { hud: None, id },
                            "finished (decode failed)",
                        );
                        self.show_ephemeral(cx, UiHud::ModelMissing, "decode failed");
                    }
                }
                FromWorker::Crashed { err } => {
                    self.on_worker_crashed(cx, &err);
                }
            }
        }
    }
}
