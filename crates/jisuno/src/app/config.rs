//! Config files: first-run embeds, read helper, poll/note/restart/secure.
//!
//! Split from `app.rs` with no behavior change (step 1).

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use gpui::App;
use jisuno_logic::hold::{CapturedKey, HoldMachine, Input, Timing};
use jisuno_logic::keybindings::{self};
use jisuno_logic::settings::{self, ConfigDir, EmbeddedFile, Settings};
use jisuno_platform::{
    keys::{Captured, KeyTap, ShouldCapture},
    permissions::{self, PermissionState},
    secure_input,
};
use jisuno_ui::hud_model::HudState as UiHud;

use super::model::model_id;
use super::state::{AppState, CONFIG_POLL_MS, SECURE_POLL_MS};
use super::tap_config_for;

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

    pub(crate) fn poll_config(&mut self, cx: &mut App) {
        if self.last_poll.elapsed() < Duration::from_millis(CONFIG_POLL_MS) {
            return;
        }
        self.last_poll = Instant::now();
        let dir = match ConfigDir::resolve() {
            Ok(dir) => dir,
            Err(e) => {
                self.log(&format!("config dir lost: {e:#}"));
                return;
            }
        };
        if self.settings_watch.changed() {
            match read_file(&dir.settings_path()).map(|t| Settings::parse(&t)) {
                Ok(Ok(settings)) => {
                    if settings != self.settings {
                        self.log("settings reloaded");
                        if settings.dictation_limit_s != self.settings.dictation_limit_s {
                            *self.machine.lock().unwrap_or_else(|e| e.into_inner()) =
                                HoldMachine::new(Timing {
                                    dictation_limit_s: u64::from(settings.dictation_limit_s),
                                    ..Timing::default()
                                });
                        }
                        self.model_name = model_id(&settings);
                        self.settings = settings;
                        self.last_config_error = None;
                    }
                }
                Ok(Err(d)) => self.note_config_error(cx, &format!("{d} · using last good config")),
                Err(e) => self.log(&format!("settings unreadable: {e:#}")),
            }
        }
        if self.keybindings_watch.changed() {
            match read_file(&dir.keybindings_path()).map(|t| keybindings::parse_with_warnings(&t)) {
                Ok(Ok((bindings, warnings))) => {
                    for w in &warnings {
                        self.log(&format!(
                            "keybindings warning: keybindings.json:{}: {}",
                            w.line, w.message
                        ));
                    }
                    if bindings != self.bindings {
                        self.log("keybindings reloaded — restarting tap");
                        self.bindings = bindings;
                        self.restart_tap();
                    }
                    self.last_config_error = None;
                }
                Ok(Err(errors)) => {
                    let first = errors.first().map(|d| {
                        format!(
                            "keybindings.json:{}: {} · using last good config",
                            d.line, d.message
                        )
                    });
                    if let Some(msg) = first {
                        self.note_config_error(cx, &msg);
                    }
                }
                Err(e) => self.log(&format!("keybindings unreadable: {e:#}")),
            }
        }
        if self.theme_watch.changed() {
            match read_file(&dir.theme_path()).map(|t| {
                serde_json::from_str::<serde_json::Value>(&t)
                    .map_err(|e| format!("invalid JSON: {e}"))
                    .and_then(|v| {
                        jisuno_ui::tokens::ThemeJson::from_value(&v).map_err(|e| format!("{e:#}"))
                    })
            }) {
                Ok(Ok(theme)) => {
                    self.show_latency = theme.show_latency;
                    self.last_config_error = None;
                    self.log("theme reloaded");
                }
                Ok(Err(message)) => self.note_config_error(
                    cx,
                    &format!("theme.json: {message} · using last good config"),
                ),
                Err(e) => self.log(&format!("theme unreadable: {e:#}")),
            }
        }
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
        let should_capture: ShouldCapture = Arc::new(move |captured: Captured| {
            let machine = machine_w.lock().unwrap_or_else(|e| e.into_inner());
            match captured {
                Captured::Escape => machine.captures(CapturedKey::Escape),
                Captured::Return => machine.captures(CapturedKey::Return),
                Captured::Chord(_) => true,
            }
        });
        match KeyTap::start(tap_config_for(&self.bindings), should_capture) {
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
