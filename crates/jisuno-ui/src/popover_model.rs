//! Menu bar popover view model (`app-ui.md` §4).
//!
//! Pure: no GPUI dependency, so Linux CI builds and tests this module. The
//! GPUI view (`popover_view`, macOS only) renders [`PopoverModel`] and emits
//! [`PopoverIntent`]s; the app agent hosts it, performs the intents (file and
//! OS access) and feeds updated models back.

/// Prefix of a Recent entry that is a voice command (not dictated text).
/// Written by the app when the entry is pushed; read back for the popover
/// row and its display text.
pub const COMMAND_RECENT_PREFIX: &str = "⌘ ";

/// Model (download/load) presence shown in the popover's Model row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelPresence {
    /// The model file is not downloaded.
    NotDownloaded,
    /// A download is running; `percent` is 0..=100.
    Downloading { percent: u32 },
    /// Downloaded, not in memory. Loads on first use.
    Cold,
    /// A load is running.
    Loading,
    /// In memory.
    Warm,
}

/// The model row state: presence plus the idle-unload policy, which the warm
/// line reads (`unloads in 10 min`; `0` shows `stays loaded`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelState {
    pub presence: ModelPresence,
    pub unload_after_idle_min: u32,
}

impl ModelState {
    /// The state dot tone and the state line copy (§4).
    pub fn line(&self) -> (&'static str, String) {
        match &self.presence {
            ModelPresence::NotDownloaded => ("caution", "Not downloaded".into()),
            ModelPresence::Downloading { percent } => {
                ("warning", format!("Downloading {percent}%"))
            }
            ModelPresence::Loading => ("warning", "Loading…".into()),
            ModelPresence::Cold => ("soft", "Not loaded · loads on first use".into()),
            ModelPresence::Warm if self.unload_after_idle_min == 0 => {
                ("ok", "Warm · ~0.9 GB · stays loaded".into())
            }
            ModelPresence::Warm => (
                "ok",
                format!(
                    "Warm · ~0.9 GB · unloads in {} min",
                    self.unload_after_idle_min
                ),
            ),
        }
    }

    /// The Model row action label (§4).
    pub fn action_label(&self) -> &'static str {
        match &self.presence {
            ModelPresence::NotDownloaded => "Download model",
            ModelPresence::Downloading { .. } => "Downloading model…",
            ModelPresence::Warm => "Unload model",
            ModelPresence::Cold | ModelPresence::Loading => "Warm model",
        }
    }

    /// Whether the action button is enabled (no intent while busy).
    pub fn action_enabled(&self) -> bool {
        !matches!(
            self.presence,
            ModelPresence::Downloading { .. } | ModelPresence::Loading
        )
    }
}

/// One permission from the Permissions section (§4, §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermissionPane {
    Microphone,
    Accessibility,
    InputMonitoring,
}

impl PermissionPane {
    pub fn name(self) -> &'static str {
        match self {
            Self::Microphone => "Microphone",
            Self::Accessibility => "Accessibility",
            Self::InputMonitoring => "Input Monitoring",
        }
    }

    /// The System Settings deep link (§6). Unverified on macOS 27.3 (§12).
    pub fn settings_link(self) -> &'static str {
        match self {
            Self::Microphone => "Privacy_Microphone",
            Self::Accessibility => "Privacy_Accessibility",
            Self::InputMonitoring => "Privacy_ListenEvent",
        }
    }
}

/// Which permissions are `Allowed` (`ok`) and which are `Not allowed`
/// (`warning`, opening their System Settings pane on click).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    pub microphone: bool,
    pub accessibility: bool,
    pub input_monitoring: bool,
}

impl Permissions {
    pub fn all_allowed(self) -> bool {
        self.microphone && self.accessibility && self.input_monitoring
    }

    pub fn rows(self) -> [(PermissionPane, bool); 3] {
        [
            (PermissionPane::Microphone, self.microphone),
            (PermissionPane::Accessibility, self.accessibility),
            (PermissionPane::InputMonitoring, self.input_monitoring),
        ]
    }
}

/// A Recent row: memory only, last 5 results (§4). Commands show as
/// `⌘ <text>`; a failed step tags the row `failed`, text that could not be
/// inserted tags it `not inserted`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentItem {
    pub text: String,
    pub command: bool,
    pub failed: bool,
    pub not_inserted: bool,
}

impl RecentItem {
    /// The row text: commands carry the `⌘` prefix (§4).
    pub fn display_text(&self) -> String {
        if self.command {
            format!("{COMMAND_RECENT_PREFIX}{}", self.text)
        } else {
            self.text.clone()
        }
    }

    /// The row tag, if any.
    pub fn tag(&self) -> Option<&'static str> {
        if self.failed {
            Some("failed")
        } else if self.not_inserted {
            Some("not inserted")
        } else {
            None
        }
    }
}

/// The popover view model. The host owns downloads, permissions and the
/// clipboard; this only names what to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PopoverModel {
    /// The model id, e.g. `parakeet-tdt-0.6b-v3-int8`.
    pub model_name: String,
    pub model: ModelState,
    pub permissions: Permissions,
    /// The app holding Secure Input, if any (§7). Draws the caution row.
    pub secure_input_holder: Option<String>,
    /// Newest first; the view shows at most 5.
    pub recent: Vec<RecentItem>,
    /// Toast copy (`Copied`), cleared by the host after showing.
    pub toast: Option<String>,
}

impl PopoverModel {
    /// Whether the menu bar icon carries the warning badge (§2, §7).
    pub fn needs_attention(&self) -> bool {
        !self.permissions.all_allowed()
            || matches!(
                self.model.presence,
                ModelPresence::NotDownloaded | ModelPresence::Downloading { .. }
            )
    }

    /// The rows the Recent section draws (at most 5, newest first).
    pub fn recent_rows(&self) -> &[RecentItem] {
        let end = self.recent.len().min(5);
        &self.recent[..end]
    }

    /// A gallery/sample model: warm, all permissions granted, some recents.
    pub fn sample() -> Self {
        Self {
            model_name: "parakeet-tdt-0.6b-v3-int8".into(),
            model: ModelState {
                presence: ModelPresence::Warm,
                unload_after_idle_min: 10,
            },
            permissions: Permissions {
                microphone: true,
                accessibility: true,
                input_monitoring: true,
            },
            secure_input_holder: None,
            recent: vec![
                RecentItem {
                    text: "the quick brown fox".into(),
                    command: false,
                    failed: false,
                    not_inserted: false,
                },
                RecentItem {
                    text: "start working".into(),
                    command: true,
                    failed: false,
                    not_inserted: false,
                },
                RecentItem {
                    text: "focus editor".into(),
                    command: true,
                    failed: true,
                    not_inserted: false,
                },
            ],
            toast: None,
        }
    }

    /// A gallery/sample model with problems: missing model, a revoked
    /// permission, secure input on, an empty recent list.
    pub fn sample_attention() -> Self {
        Self {
            model_name: "parakeet-tdt-0.6b-v3-int8".into(),
            model: ModelState {
                presence: ModelPresence::NotDownloaded,
                unload_after_idle_min: 10,
            },
            permissions: Permissions {
                microphone: true,
                accessibility: false,
                input_monitoring: true,
            },
            secure_input_holder: Some("Terminal".into()),
            recent: Vec::new(),
            toast: None,
        }
    }
}

/// Typed intents the host must implement. The view never touches files or
/// the OS itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PopoverIntent {
    WarmModel,
    UnloadModel,
    DownloadModel,
    /// Copy the recent row's text (`Copied` toast on success).
    CopyRecent(usize),
    /// A `Not allowed` permission row: open its System Settings pane.
    OpenPermissionPane(PermissionPane),
    OpenSettings,
    Quit,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_lines_match_the_spec() {
        let warm = ModelState {
            presence: ModelPresence::Warm,
            unload_after_idle_min: 10,
        };
        assert_eq!(
            warm.line(),
            ("ok", "Warm · ~0.9 GB · unloads in 10 min".into())
        );
        assert_eq!(warm.action_label(), "Unload model");
        let stays = ModelState {
            presence: ModelPresence::Warm,
            unload_after_idle_min: 0,
        };
        assert_eq!(stays.line().1, "Warm · ~0.9 GB · stays loaded");
        let cold = ModelState {
            presence: ModelPresence::Cold,
            unload_after_idle_min: 10,
        };
        assert_eq!(
            cold.line(),
            ("soft", "Not loaded · loads on first use".into())
        );
        assert_eq!(cold.action_label(), "Warm model");
        let loading = ModelState {
            presence: ModelPresence::Loading,
            unload_after_idle_min: 10,
        };
        assert_eq!(loading.line(), ("warning", "Loading…".into()));
        assert!(!loading.action_enabled());
        let missing = ModelState {
            presence: ModelPresence::NotDownloaded,
            unload_after_idle_min: 10,
        };
        assert_eq!(missing.line(), ("caution", "Not downloaded".into()));
        assert_eq!(missing.action_label(), "Download model");
        let downloading = ModelState {
            presence: ModelPresence::Downloading { percent: 42 },
            unload_after_idle_min: 10,
        };
        assert_eq!(downloading.line().1, "Downloading 42%");
        assert_eq!(downloading.action_label(), "Downloading model…");
        assert!(!downloading.action_enabled());
    }

    #[test]
    fn attention_and_permissions() {
        assert!(!PopoverModel::sample().needs_attention());
        assert!(PopoverModel::sample_attention().needs_attention());
        let all = Permissions {
            microphone: true,
            accessibility: true,
            input_monitoring: true,
        };
        assert!(all.all_allowed());
        assert_eq!(all.rows().len(), 3);
        assert_eq!(
            PermissionPane::Microphone.settings_link(),
            "Privacy_Microphone"
        );
        assert_eq!(
            PermissionPane::Accessibility.settings_link(),
            "Privacy_Accessibility"
        );
        assert_eq!(
            PermissionPane::InputMonitoring.settings_link(),
            "Privacy_ListenEvent"
        );
    }

    #[test]
    fn recent_rows_cap_at_five_and_tag() {
        let mut model = PopoverModel::sample();
        for n in 0..7 {
            model.recent.push(RecentItem {
                text: format!("utterance {n}"),
                command: n % 2 == 0,
                failed: n == 1,
                not_inserted: n == 2,
            });
        }
        assert_eq!(model.recent_rows().len(), 5);
        let command = RecentItem {
            text: "start working".into(),
            command: true,
            failed: false,
            not_inserted: false,
        };
        assert_eq!(command.display_text(), "⌘ start working");
        assert_eq!(command.tag(), None);
        let failed = RecentItem {
            text: "x".into(),
            command: true,
            failed: true,
            not_inserted: false,
        };
        assert_eq!(failed.tag(), Some("failed"));
        let pending = RecentItem {
            text: "y".into(),
            command: false,
            failed: false,
            not_inserted: true,
        };
        assert_eq!(pending.tag(), Some("not inserted"));
        assert_eq!(pending.display_text(), "y");
    }
}
