//! Menu bar chrome: icon state, model line, permission rows.
//!
//! Split from `app.rs` with no behavior change (step 1).

use std::sync::{Mutex, OnceLock};

use jisuno_platform::{permissions::Pane, status_item::IconState};

use super::model::ModelState;
use super::socket::state_name;
use super::state::AppState;

// --- Menu bar chrome ---

/// Last icon state pushed to the menu bar: `set_icon` rewrites the button
/// title/tooltip every call, so only push on change. Process-global: one
/// app instance owns the status item.
static LAST_ICON: OnceLock<Mutex<Option<(IconState, bool, bool)>>> = OnceLock::new();

impl AppState {
    // --- Menu bar chrome ---

    pub(crate) fn refresh_chrome(&mut self) {
        self.refresh_socket_snapshot();
        let warning = self.warning_active();
        let state = if warning {
            IconState::Error
        } else if matches!(self.model_state, ModelState::Loading { gen } if gen == self.model_gen) {
            IconState::Loading
        } else if let Some(l) = &self.listening {
            if l.dictate {
                IconState::ListeningDictate
            } else {
                IconState::ListeningCommand
            }
        } else {
            IconState::Idle {
                model_warm: self.model_state == ModelState::Warm,
            }
        };
        let icon = (state, warning, self.secure_now);
        let changed = LAST_ICON
            .get_or_init(|| Mutex::new(None))
            .lock()
            .map(|mut last| {
                if *last == Some(icon) {
                    false
                } else {
                    *last = Some(icon);
                    true
                }
            })
            .unwrap_or(true);
        if changed {
            self.status.set_icon(state, warning, self.secure_now);
        }

        let line = match self.model_state {
            ModelState::Warm => {
                let mins = self.settings.unload_after_idle_min;
                if mins == 0 {
                    "Warm · ~0.9 GB · stays loaded".to_string()
                } else {
                    format!("Warm · ~0.9 GB · unloads in {mins} min")
                }
            }
            ModelState::Loading { .. } => "Loading…".to_string(),
            ModelState::Unloaded if self.model_present => {
                "Not loaded · loads on first use".to_string()
            }
            ModelState::Unloaded => "Not downloaded".to_string(),
            ModelState::Failed => "Load failed".to_string(),
        };
        if line != self.model_line {
            self.model_line = line.clone();
            self.status.set_model_line(&line);
        }

        let perms = self.perm_granted;
        let panes = [Pane::Microphone, Pane::Accessibility, Pane::InputMonitoring];
        for (i, (granted, pane)) in perms.into_iter().zip(panes).enumerate() {
            let text = format!(
                "{} — {}",
                match pane {
                    Pane::Microphone => "Microphone",
                    Pane::Accessibility => "Accessibility",
                    Pane::InputMonitoring => "Input Monitoring",
                },
                if granted { "Allowed" } else { "Not allowed" }
            );
            if text != self.perm_line[i] {
                self.perm_line[i] = text;
                self.status.set_permission(pane, granted);
            }
        }
    }

    pub(crate) fn refresh_socket_snapshot(&mut self) {
        let state = state_name(&self.model_state).to_string();
        let busy = !self.can_unload();
        if let Ok(mut snapshot) = self.socket_snapshot.lock() {
            if snapshot.state != state || snapshot.model != self.model_name || snapshot.busy != busy
            {
                snapshot.state = state;
                snapshot.model.clone_from(&self.model_name);
                snapshot.busy = busy;
            }
        }
    }

    pub(crate) fn warning_active(&self) -> bool {
        if self.last_config_error.is_some() {
            return true;
        }
        if self.model_state == ModelState::Failed {
            return true;
        }
        if !self.model_present {
            return true;
        }
        self.perm_granted != [true, true, true]
    }
}
