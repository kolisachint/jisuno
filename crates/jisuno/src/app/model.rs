//! Model lifecycle: load/cancel/unload, idle unload, worker drain.
//!
//! Split from `app.rs` with no behavior change (step 1).

use std::time::{Duration, Instant};

use gpui::App;
use jisuno_logic::hold::Input;
use jisuno_logic::settings::{self, Settings};
use jisuno_ui::hud_model::HudState as UiHud;

use super::state::AppState;
use super::worker::{FromWorker, ToWorker};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelState {
    Unloaded,
    Loading { gen: u64 },
    Warm,
    Failed,
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

    pub(crate) fn unload_model(&mut self, why: &str) {
        if self.model_state == ModelState::Warm {
            let _ = self.to_worker.send(ToWorker::Unload);
            self.model_state = ModelState::Unloaded;
            self.machine
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .set_model_warm(false);
            self.log(&format!("model unloaded ({why})"));
        } else {
            self.log(&format!("unload ({why}): nothing loaded"));
        }
    }

    pub(crate) fn poll_idle_unload(&mut self) {
        let mins = self.settings.unload_after_idle_min;
        if mins == 0
            || self.model_state != ModelState::Warm
            || self.pending.is_some()
            || self.mic_open
        {
            return;
        }
        if self.last_active.elapsed() >= Duration::from_secs(u64::from(mins) * 60) {
            self.unload_model("idle");
        }
    }

    pub(crate) fn drain_worker(&mut self, cx: &mut App) {
        while let Ok(msg) = self.from_worker.try_recv() {
            match msg {
                FromWorker::Warm { gen } => {
                    if gen != self.model_gen {
                        continue;
                    }
                    self.model_state = ModelState::Warm;
                    self.last_active = Instant::now();
                    self.machine
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .handle(Input::ModelWarm, self.now_ms());
                    self.log("model warm");
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
                FromWorker::Transcript {
                    id,
                    text,
                    decode_ms,
                } => {
                    self.on_transcript(cx, id, text, decode_ms);
                }
                FromWorker::NotReady { id } => {
                    self.log(&format!("worker not ready for {id}"));
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
                    if self.pending.take().is_some() {
                        self.input(
                            cx,
                            Input::Finished { hud: None, id },
                            "finished (decode failed)",
                        );
                        self.show_ephemeral(cx, UiHud::ModelMissing, "decode failed");
                    }
                }
            }
        }
    }
}
