//! Config files: first-run embeds, read helper, poll/note/restart/secure.
//!
//! Split from `app.rs` with no behavior change (step 1).

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use gpui::App;
use jisuno_logic::hold::{CapturedKey, HoldMachine, Input, Timing};
use jisuno_logic::keybindings::{self, Keybindings};
use jisuno_logic::settings::{self, ConfigDir, EmbeddedFile, Settings};
use jisuno_logic::voice_control::{self, Config};
use jisuno_platform::{
    keys::{Captured, KeyTap, ShouldCapture},
    permissions::{self, PermissionState},
    secure_input,
};
use jisuno_ui::hud_model::HudState as UiHud;

use super::model::model_id;
use super::state::{AppState, SECURE_POLL_MS};
use super::tap_config_for;

/// Dictation-limit change seen while a session is live: applied when the
/// machine is back to Idle (never mid-hold). 0 = none (the valid range is
/// 10..=600, so 0 is never a real limit). Process-global: one app instance
/// owns the mic, and `AppState` gains no field for it (see mod.rs).
static PENDING_DICTATION_LIMIT_S: AtomicU64 = AtomicU64::new(0);

/// Take a deferred timing change once the machine is Idle: returns the
/// pending limit and clears the slot, or `None` while a session is live.
/// Pure apart from the process-global slot; unit-tested below.
pub(crate) fn pending_timing_take(session_active: bool) -> Option<u64> {
    if session_active {
        return None;
    }
    match PENDING_DICTATION_LIMIT_S.swap(0, Ordering::SeqCst) {
        0 => None,
        limit_s => Some(limit_s),
    }
}

// --- Startup: config files, permissions -----------------------------------

pub(crate) fn embedded_app_files() -> Vec<EmbeddedFile> {
    let mut files: Vec<EmbeddedFile> = settings::embedded_settings_files().to_vec();
    files.push(EmbeddedFile {
        name: "keybindings.json",
        contents: include_str!("../../../jisuno-logic/defaults/keybindings.json"),
    });
    files.push(EmbeddedFile {
        name: "schemas/keybindings.schema.json",
        contents: include_str!("../../../jisuno-logic/schemas/keybindings.schema.json"),
    });
    files.push(EmbeddedFile {
        name: "theme.json",
        contents: include_str!("../../../jisuno-logic/defaults/theme.json"),
    });
    files.push(EmbeddedFile {
        name: "schemas/theme.schema.json",
        contents: include_str!("../../../jisuno-logic/schemas/theme.schema.json"),
    });
    files.push(EmbeddedFile {
        name: "voice-control-config.json",
        contents: include_str!("../../../jisuno-logic/defaults/voice-control-config.json"),
    });
    files.push(EmbeddedFile {
        name: "schemas/voice-control-config.schema.json",
        contents: include_str!("../../../jisuno-logic/schemas/voice-control-config.schema.json"),
    });
    files
}

pub(crate) fn read_file(path: &std::path::Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}

pub(crate) fn stamp(start: Instant, msg: &str) {
    let e = start.elapsed();
    eprintln!(
        "[{:02}:{:02}.{:03}] {msg}",
        e.as_secs() / 60,
        e.as_secs() % 60,
        e.subsec_millis()
    );
}

// --- Polls ---

impl AppState {
    // --- Polls ---

    /// Drain the background watcher: re-parse each changed file, keep last
    /// good on error (first error shows once via `LastGood`), apply live.
    /// `settings::LastGood` owns the error state; other config types convert
    /// their diagnostics into it.
    pub(crate) fn drain_watch(&mut self, cx: &mut App) {
        let mut changed = Vec::new();
        while let Ok(path) = self.watch_rx.try_recv() {
            changed.push(path);
        }
        if changed.is_empty() {
            return;
        }
        let dir = match ConfigDir::resolve() {
            Ok(dir) => dir,
            Err(e) => {
                self.log(&format!("config dir lost: {e:#}"));
                return;
            }
        };
        let settings_path = dir.settings_path();
        let keybindings_path = dir.keybindings_path();
        let theme_path = dir.theme_path();
        let voice_path = dir.voice_control_path();
        for path in changed {
            if path == settings_path {
                self.reload_settings(cx, &dir);
            } else if path == keybindings_path {
                self.reload_keybindings(cx, &dir);
            } else if path == theme_path {
                self.reload_theme(cx, &dir);
            } else if path == voice_path {
                self.reload_voice(cx, &dir);
            }
        }
    }

    pub(crate) fn reload_settings(&mut self, cx: &mut App, dir: &ConfigDir) {
        let result = match read_file(&dir.settings_path()) {
            Ok(text) => Settings::parse(&text),
            Err(e) => {
                self.log(&format!("settings unreadable: {e:#}"));
                return;
            }
        };
        if let Some(d) = self.settings_good.apply(result) {
            self.note_config_error(
                cx,
                &format!(
                    "settings.json:{}: {} · using last good config",
                    d.line, d.message
                ),
            );
            return;
        }
        let fresh = self.settings_good.current().clone();
        if fresh != self.settings {
            self.log("settings reloaded");
            let limit_s = u64::from(fresh.dictation_limit_s);
            if limit_s != u64::from(self.settings.dictation_limit_s) {
                // Never swap the machine mid-hold: the timing change waits
                // for Idle, and the load state is carried over, not reset.
                if self.session_active() {
                    PENDING_DICTATION_LIMIT_S.store(limit_s, Ordering::SeqCst);
                    self.log("dictation limit change deferred until idle");
                } else {
                    self.apply_timing(limit_s);
                }
            }
            self.model_name = model_id(&fresh);
            self.settings = fresh;
        }
        self.last_config_error = None;
    }

    /// A live session (or a pending reply/confirm) means the machine is not
    /// Idle: timing changes wait.
    pub(crate) fn session_active(&self) -> bool {
        self.mic_open || self.pending.is_some() || self.confirming.is_some()
    }

    /// Rebuild the machine with a new dictation limit, carrying the load
    /// state (warm/unloaded) instead of resetting it.
    pub(crate) fn apply_timing(&mut self, limit_s: u64) {
        let warm = self
            .machine
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_model_warm();
        *self.machine.lock().unwrap_or_else(|e| e.into_inner()) = HoldMachine::new(Timing {
            dictation_limit_s: limit_s,
            ..Timing::default()
        });
        self.machine
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_model_warm(warm);
        PENDING_DICTATION_LIMIT_S.store(0, Ordering::SeqCst);
        self.log(&format!("dictation limit now {limit_s}s"));
    }

    /// Apply a deferred [`Self::reload_settings`] timing change once Idle.
    pub(crate) fn apply_pending_timing(&mut self) {
        if let Some(limit_s) = pending_timing_take(self.session_active()) {
            self.apply_timing(limit_s);
        }
    }

    pub(crate) fn reload_keybindings(&mut self, cx: &mut App, dir: &ConfigDir) {
        let result: Result<Keybindings, jisuno_logic::settings::Diagnostic> =
            match read_file(&dir.keybindings_path()) {
                Ok(text) => match keybindings::parse_with_warnings(&text) {
                    Ok((bindings, warnings)) => {
                        for w in &warnings {
                            self.log(&format!(
                                "keybindings warning: keybindings.json:{}: {}",
                                w.line, w.message
                            ));
                        }
                        Ok(bindings)
                    }
                    Err(errors) => Err(errors.into_iter().next().map_or_else(
                        || jisuno_logic::settings::Diagnostic::new(1, "invalid keybindings file"),
                        |d| jisuno_logic::settings::Diagnostic::new(d.line, d.message),
                    )),
                },
                Err(e) => {
                    self.log(&format!("keybindings unreadable: {e:#}"));
                    return;
                }
            };
        if let Some(d) = self.bindings_good.apply(result) {
            self.note_config_error(
                cx,
                &format!(
                    "keybindings.json:{}: {} · using last good config",
                    d.line, d.message
                ),
            );
            return;
        }
        let fresh = self.bindings_good.current().clone();
        if fresh != self.bindings {
            self.log("keybindings reloaded — restarting tap");
            self.bindings = fresh;
            self.restart_tap();
        }
        self.last_config_error = None;
    }

    pub(crate) fn reload_theme(&mut self, cx: &mut App, dir: &ConfigDir) {
        let result: Result<jisuno_ui::tokens::ThemeJson, jisuno_logic::settings::Diagnostic> =
            match read_file(&dir.theme_path()) {
                Ok(text) => {
                    match serde_json::from_str::<serde_json::Value>(&text)
                        .map_err(|e| format!("invalid JSON: {e}"))
                        .and_then(|v| {
                            jisuno_ui::tokens::ThemeJson::from_value(&v)
                                .map_err(|e| format!("{e:#}"))
                        }) {
                        Ok(theme) => Ok(theme),
                        Err(message) => Err(jisuno_logic::settings::Diagnostic::new(1, message)),
                    }
                }
                Err(e) => {
                    self.log(&format!("theme unreadable: {e:#}"));
                    return;
                }
            };
        if let Some(d) = self.theme_good.apply(result) {
            self.note_config_error(
                cx,
                &format!(
                    "theme.json:{}: {} · using last good config",
                    d.line, d.message
                ),
            );
            return;
        }
        self.show_latency = self.theme_good.current().show_latency;
        self.last_config_error = None;
        self.log("theme reloaded");
    }

    pub(crate) fn reload_voice(&mut self, cx: &mut App, dir: &ConfigDir) {
        let result: Result<Config, jisuno_logic::settings::Diagnostic> =
            match read_file(&dir.voice_control_path()) {
                Ok(text) => match voice_control::parse(&text) {
                    Ok((config, warnings)) => {
                        for w in &warnings {
                            self.log(&format!(
                                "voice-control warning: {}",
                                w.render("voice-control-config.json")
                            ));
                        }
                        Ok(config)
                    }
                    Err(errors) => Err(errors.into_iter().next().map_or_else(
                        || {
                            jisuno_logic::settings::Diagnostic::new(
                                1,
                                "invalid voice-control config",
                            )
                        },
                        |d| jisuno_logic::settings::Diagnostic::new(d.line, d.message),
                    )),
                },
                Err(e) => {
                    self.log(&format!("voice-control unreadable: {e:#}"));
                    return;
                }
            };
        if let Some(d) = self.voice_good.apply(result) {
            self.note_config_error(
                cx,
                &format!(
                    "voice-control-config.json:{}: {} · using last good config",
                    d.line, d.message
                ),
            );
            return;
        }
        let fresh = self.voice_good.current().clone();
        if fresh != self.voice_config {
            self.voice_config = fresh;
            self.log("voice-control config reloaded");
        }
        self.last_config_error = None;
    }

    pub(crate) fn note_config_error(&mut self, cx: &mut App, msg: &str) {
        if self.last_config_error.as_deref() == Some(msg) {
            return;
        }
        self.last_config_error = Some(msg.to_string());
        self.log(&format!("config error: {msg}"));
        let location = msg.split(':').next().unwrap_or("config").to_string();
        self.show_ephemeral(
            cx,
            UiHud::ConfigError {
                location,
                message: msg.to_string(),
            },
            "config error",
        );
    }

    pub(crate) fn poll_secure(&mut self, _cx: &mut App) {
        if self.last_secure_poll.elapsed() < Duration::from_millis(SECURE_POLL_MS) {
            return;
        }
        self.last_secure_poll = Instant::now();
        self.perm_granted = [
            self.mic_satisfied || permissions::microphone() == PermissionState::Granted,
            permissions::accessibility() == PermissionState::Granted,
            permissions::input_monitoring() == PermissionState::Granted,
        ];
        self.model_present = self.model_files_present();
        let on = secure_input::is_secure_input_enabled();
        if on && !self.secure_now {
            self.secure_now = true;
            self.log("secure input on — resyncing");
            self.input_no_cx(Input::Resync, "resync (secure input)");
        } else if !on && self.secure_now {
            self.secure_now = false;
            self.log("secure input off — keys live again");
        }
    }

    pub(crate) fn restart_tap(&mut self) {
        if let Some(tap) = self.tap.take() {
            if let Err(e) = tap.shutdown() {
                self.log(&format!("tap shutdown: {e:#}"));
            }
        }
        let machine_w = Arc::clone(&self.machine);
        let tap_capture_w = Arc::clone(&self.tap_capture);
        let should_capture: ShouldCapture = Arc::new(move |captured: Captured| {
            if tap_capture_w.load(Ordering::SeqCst) {
                return true;
            }
            let machine = machine_w.lock().unwrap_or_else(|e| e.into_inner());
            match captured {
                Captured::Escape => machine.captures(CapturedKey::Escape),
                Captured::Return => machine.captures(CapturedKey::Return),
                Captured::Chord(_) => true,
            }
        });
        match KeyTap::start(
            tap_config_for(&self.bindings, self.key_capturing.is_some()),
            should_capture,
        ) {
            Ok((tap, rx)) => {
                self.tap = Some(tap);
                self.tap_rx = Some(rx);
                self.log("tap restarted");
            }
            Err(e) => {
                self.tap_rx = None;
                self.log(&format!("tap restart failed: {e:#}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{pending_timing_take, PENDING_DICTATION_LIMIT_S};
    use std::sync::atomic::Ordering;

    /// A timing change waits while a session is live and is taken exactly
    /// once when Idle.
    #[test]
    fn deferred_timing_waits_for_idle() {
        PENDING_DICTATION_LIMIT_S.store(0, Ordering::SeqCst);
        assert_eq!(pending_timing_take(false), None);
        PENDING_DICTATION_LIMIT_S.store(90, Ordering::SeqCst);
        assert_eq!(pending_timing_take(true), None);
        assert_eq!(pending_timing_take(false), Some(90));
        assert_eq!(pending_timing_take(false), None);
    }
}
