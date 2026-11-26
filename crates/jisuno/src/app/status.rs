//! Menu bar chrome: icon state, model line, permission rows.
//!
//! Split from `app.rs` with no behavior change (step 1).

use jisuno_platform::{permissions::Pane, status_item::IconState};

use super::model::ModelState;
use super::state::AppState;

// --- Menu bar chrome ---

impl AppState {
    // --- Menu bar chrome ---

    pub(crate) fn refresh_chrome(&mut self) {
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
        self.status.set_icon(state, warning, self.secure_now);

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
