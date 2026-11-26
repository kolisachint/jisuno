//! First-run Welcome window view model (`app-ui.md` §6).
//!
//! Pure: no GPUI dependency, so Linux CI builds and tests this module. The
//! GPUI view (`welcome_view`, macOS only) renders [`WelcomeModel`] and emits
//! [`WelcomeIntent`]s; the app agent hosts it, performs the intents and feeds
//! updated models back. The window re-checks permissions live (every second
//! and on focus); starting does not wait for the model.

/// One of the three first-run permissions (§6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PermissionKind {
    Microphone,
    Accessibility,
    InputMonitoring,
}

impl PermissionKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Microphone => "Microphone",
            Self::Accessibility => "Accessibility",
            Self::InputMonitoring => "Input Monitoring",
        }
    }

    /// The reason caption under the permission name (§6).
    pub fn reason(self) -> &'static str {
        match self {
            Self::Microphone => "Records only while you hold a key.",
            Self::Accessibility => "Inserts text and moves windows.",
            Self::InputMonitoring => "Lets the hold keys work in every app.",
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

/// The three permissions in §6 order.
pub const PERMISSIONS: [PermissionKind; 3] = [
    PermissionKind::Microphone,
    PermissionKind::Accessibility,
    PermissionKind::InputMonitoring,
];

/// The speech-model download: Parakeet TDT 0.6B v3, int8, about 650 MB (§6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadProgress {
    pub percent: u32,
    pub downloaded_mb: u32,
    pub total_mb: u32,
}

impl DownloadProgress {
    /// `Downloading 42% · 273 of 650 MB` (§6).
    pub fn line(&self) -> String {
        format!(
            "Downloading {}% · {} of {} MB",
            self.percent, self.downloaded_mb, self.total_mb
        )
    }
}

/// The total download size in MB (§6).
pub const MODEL_TOTAL_MB: u32 = 650;

/// The Welcome window model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WelcomeModel {
    /// In [`PERMISSIONS`] order.
    pub allowed: [bool; 3],
    /// The four config files + `schemas/` were written first (§6.1).
    pub files_written: bool,
    /// The model file is fully downloaded.
    pub model_present: bool,
    /// A download in progress, if any.
    pub download: Option<DownloadProgress>,
}

impl WelcomeModel {
    /// `Start Jisuno` is enabled only when all three permissions are
    /// `Allowed` (§6 footer).
    pub fn can_start(&self) -> bool {
        self.allowed.iter().all(|allowed| *allowed)
    }

    /// The footer caption when starting is blocked, if any.
    pub fn start_hint(&self) -> Option<&'static str> {
        if self.can_start() {
            None
        } else {
            Some("Allow the three permissions to start.")
        }
    }

    /// The model section copy (§6.3).
    pub fn model_line(&self) -> (Option<&'static str>, String) {
        if let Some(download) = &self.download {
            (None, download.line())
        } else if self.model_present {
            (Some("ok"), "Downloaded".into())
        } else {
            (
                None,
                "Not downloaded. Dictation works once the download finishes.".into(),
            )
        }
    }

    /// A gallery/sample model: two permissions granted, files written, no
    /// model yet.
    pub fn sample() -> Self {
        Self {
            allowed: [true, true, false],
            files_written: true,
            model_present: false,
            download: None,
        }
    }

    /// A gallery/sample model mid-download with all permissions granted.
    pub fn sample_downloading() -> Self {
        Self {
            allowed: [true, true, true],
            files_written: true,
            model_present: false,
            download: Some(DownloadProgress {
                percent: 42,
                downloaded_mb: 273,
                total_mb: MODEL_TOTAL_MB,
            }),
        }
    }
}

/// Typed intents the host must implement. The view never touches files or
/// the OS itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WelcomeIntent {
    /// Open the permission's System Settings pane.
    OpenPermissionPane(PermissionKind),
    DownloadModel,
    /// Close Welcome; the app moves to the menu bar.
    Start,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_copy_matches_the_spec() {
        assert_eq!(
            PermissionKind::Microphone.reason(),
            "Records only while you hold a key."
        );
        assert_eq!(
            PermissionKind::Accessibility.reason(),
            "Inserts text and moves windows."
        );
        assert_eq!(
            PermissionKind::InputMonitoring.reason(),
            "Lets the hold keys work in every app."
        );
        assert_eq!(
            PermissionKind::InputMonitoring.settings_link(),
            "Privacy_ListenEvent"
        );
        assert_eq!(PERMISSIONS.len(), 3);
    }

    #[test]
    fn start_needs_all_three_permissions() {
        assert!(!WelcomeModel::sample().can_start());
        assert_eq!(
            WelcomeModel::sample().start_hint(),
            Some("Allow the three permissions to start.")
        );
        let mut ready = WelcomeModel::sample();
        ready.allowed = [true, true, true];
        assert!(ready.can_start());
        assert_eq!(ready.start_hint(), None);
    }

    #[test]
    fn model_lines() {
        let fresh = WelcomeModel::sample();
        assert_eq!(
            fresh.model_line().1,
            "Not downloaded. Dictation works once the download finishes."
        );
        let downloading = WelcomeModel::sample_downloading();
        assert_eq!(
            downloading.model_line().1,
            "Downloading 42% · 273 of 650 MB"
        );
        let mut done = WelcomeModel::sample();
        done.model_present = true;
        assert_eq!(done.model_line(), (Some("ok"), "Downloaded".into()));
        assert_eq!(MODEL_TOTAL_MB, 650);
    }
}
