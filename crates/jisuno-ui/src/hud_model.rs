//! The HUD state model: every HUD state from `app-ui.md` §3.3 plus the §7
//! error states that draw a HUD line, and the exact copy each one shows.
//!
//! Pure: no GPUI dependency, so Linux CI builds and tests this module.
//! The GPUI view (`hud_view`, macOS only) renders [`HudModel`].

use crate::tokens::format_countdown;

/// Dictate or command mode: always also a word, never colour alone (§1.1, §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Dictate,
    Command,
}

/// Every HUD state from `app-ui.md` §3.3 (H1..H18) and §7.
///
/// Listening states carry their modifiers (loading bar, partial text,
/// countdown, notes): the prototype draws them as extra lines under one
/// listening row, and one HUD state replaces the last (§3.2).
#[derive(Debug, Clone, PartialEq)]
pub enum HudState {
    /// H1: hold Right ⌥ past 150 ms, or ⌃⌥Space. `level` is 0..=1.
    ListeningDictate {
        level: f32,
        hands_free: bool,
        loading: Option<f32>,
        partial: Option<String>,
        countdown_ms: Option<u64>,
        bluetooth: bool,
        missing_device: Option<String>,
    },
    /// H2: hold Right ⌘ past 150 ms.
    ListeningCommand {
        level: f32,
        loading: Option<f32>,
        partial: Option<String>,
    },
    /// H6: key-up or hands-free end. `finishing_load` selects the meta copy.
    Decoding { mode: Mode, finishing_load: bool },
    /// H7: dictation inserted. `latency_ms` is `Some` only when
    /// `theme.json` `hud.show_latency` is on.
    Inserted { words: u32, latency_ms: Option<u64> },
    /// H8: a command's steps all succeeded.
    RanCommand { label: String },
    /// H9: Esc while listening or confirming.
    Cancelled,
    /// H10: a command with `confirm: true`. Waits for Return/Esc.
    Confirm { command: String, first_step: String },
    /// H10 timeout: 6 s with no answer.
    ConfirmExpired,
    /// H11: the matcher finds nothing close enough.
    NoMatch { suggestion: String },
    /// Runner `Resolution::NoMatch`: no eligible command, nothing runs.
    /// HUD copy: `No command matched`.
    NoCommandMatched,
    /// Runner `Resolution::Failed` after a match (slot fill failure).
    /// HUD copy: `Could not run "<label>": <reason>`.
    ResolveFailed { label: String, reason: String },
    /// H12: a step fails; earlier steps stay done.
    StepFailed {
        step: u32,
        total: u32,
        action: String,
        reason: String,
    },
    /// H13: a file fails validation on reload. `location` is `file:line`.
    ConfigError { location: String, message: String },
    /// H14: the focused app quit or its window closed before key-up.
    FocusLost,
    /// H15: a hold or key while Secure Input is on.
    SecureInput { app: String },
    /// §7: no microphone opened. No level, no listening row.
    NoMicrophone,
    /// §7: microphone permission revoked.
    MicrophoneOff,
    /// §7: Accessibility revoked; the text goes to Recent.
    InsertBlocked,
    /// §7: the model is not downloaded. The mic does not open.
    ModelMissing,
    /// §7: a model download already runs. The mic does not open.
    ModelDownloading { percent: u32 },
    /// H18: a load finished with no hold running.
    ModelReady,
}

/// The HUD accent: fills, glyphs and chips. The HUD is always dark (§1.1),
/// so these are the `hud_*` tokens, not the palette accents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HudAccent {
    Dictate,
    Command,
    Ok,
    Error,
    Neutral,
}

/// The HUD glyph: level bars while listening, a spinner while decoding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HudIcon {
    Level(f32),
    Spinner,
    None,
}

/// What the HUD view draws: the mode/chip line.
///
/// `primary_text` is the mode label or chip copy; `secondary_text` is the
/// right-aligned meta (`esc to cancel`, the countdown, latency). `progress`
/// is 0..=1 for the thin load/download bars. The optional second line
/// (partial text, confirm body, notes) comes from [`detail_lines`].
#[derive(Debug, Clone, PartialEq)]
pub struct HudModel {
    pub accent: HudAccent,
    pub icon: HudIcon,
    pub primary_text: String,
    pub secondary_text: Option<String>,
    pub progress: Option<f32>,
    pub dismiss_after_ms: Option<u64>,
}

/// Map a state to its model: the exact copy strings from `app-ui.md` §3.3/§7.
pub fn render_model(state: &HudState) -> HudModel {
    match state {
        HudState::ListeningDictate {
            level,
            hands_free,
            loading,
            countdown_ms,
            ..
        } => {
            let meta = match countdown_ms {
                Some(left) => format_countdown(*left),
                None if *hands_free => "hands-free · esc to cancel".into(),
                None => "esc to cancel".into(),
            };
            HudModel {
                accent: HudAccent::Dictate,
                icon: HudIcon::Level(level.clamp(0.0, 1.0)),
                primary_text: "Dictate".into(),
                secondary_text: Some(meta),
                progress: *loading,
                dismiss_after_ms: None,
            }
        }
        HudState::ListeningCommand { level, loading, .. } => HudModel {
            accent: HudAccent::Command,
            icon: HudIcon::Level(level.clamp(0.0, 1.0)),
            primary_text: "Command".into(),
            secondary_text: Some("esc to cancel".into()),
            progress: *loading,
            dismiss_after_ms: None,
        },
        HudState::Decoding {
            mode,
            finishing_load,
        } => HudModel {
            accent: match mode {
                Mode::Dictate => HudAccent::Dictate,
                Mode::Command => HudAccent::Command,
            },
            icon: HudIcon::Spinner,
            primary_text: match mode {
                Mode::Dictate => "Dictate".into(),
                Mode::Command => "Command".into(),
            },
            secondary_text: Some(if *finishing_load {
                "finishing model load".into()
            } else {
                "decoding".into()
            }),
            progress: None,
            dismiss_after_ms: None,
        },
        HudState::Inserted { words, latency_ms } => HudModel {
            accent: HudAccent::Ok,
            icon: HudIcon::None,
            primary_text: format!(
                "Inserted {words} {}",
                if *words == 1 { "word" } else { "words" }
            ),
            secondary_text: latency_ms.map(|ms| format!("{ms} ms")),
            progress: None,
            dismiss_after_ms: Some(1200),
        },
        HudState::RanCommand { label } => HudModel {
            accent: HudAccent::Ok,
            icon: HudIcon::None,
            primary_text: format!("Ran \"{label}\""),
            secondary_text: None,
            progress: None,
            dismiss_after_ms: Some(1300),
        },
        HudState::Cancelled => HudModel {
            accent: HudAccent::Neutral,
            icon: HudIcon::None,
            primary_text: "Cancelled".into(),
            secondary_text: None,
            progress: None,
            dismiss_after_ms: Some(700),
        },
        HudState::Confirm { .. } => HudModel {
            accent: HudAccent::Command,
            icon: HudIcon::None,
            primary_text: "Command".into(),
            secondary_text: None,
            progress: None,
            dismiss_after_ms: None,
        },
        HudState::ConfirmExpired => HudModel {
            accent: HudAccent::Neutral,
            icon: HudIcon::None,
            primary_text: "Not run".into(),
            secondary_text: None,
            progress: None,
            dismiss_after_ms: Some(1000),
        },
        HudState::NoMatch { suggestion } => HudModel {
            accent: HudAccent::Neutral,
            icon: HudIcon::None,
            primary_text: format!("Did you mean \"{suggestion}\"?"),
            secondary_text: None,
            progress: None,
            dismiss_after_ms: Some(2500),
        },
        HudState::NoCommandMatched => HudModel {
            accent: HudAccent::Neutral,
            icon: HudIcon::None,
            primary_text: "No command matched".into(),
            secondary_text: None,
            progress: None,
            dismiss_after_ms: Some(2500),
        },
        HudState::ResolveFailed { label, reason } => HudModel {
            accent: HudAccent::Error,
            icon: HudIcon::None,
            primary_text: format!("Could not run \"{label}\": {reason}"),
            secondary_text: None,
            progress: None,
            dismiss_after_ms: Some(4000),
        },
        HudState::StepFailed {
            step,
            total,
            action,
            reason,
        } => HudModel {
            accent: HudAccent::Error,
            icon: HudIcon::None,
            primary_text: format!("Step {step} of {total} failed: {action} · {reason}"),
            secondary_text: None,
            progress: None,
            dismiss_after_ms: Some(4000),
        },
        HudState::ConfigError { location, message } => HudModel {
            accent: HudAccent::Error,
            icon: HudIcon::None,
            primary_text: format!("{location}: {message} · using last good config"),
            secondary_text: None,
            progress: None,
            dismiss_after_ms: Some(6000),
        },
        HudState::FocusLost => HudModel {
            accent: HudAccent::Error,
            icon: HudIcon::None,
            primary_text: "Focus lost: text in Recent".into(),
            secondary_text: None,
            progress: None,
            dismiss_after_ms: Some(4000),
        },
        HudState::SecureInput { app } => HudModel {
            accent: HudAccent::Error,
            icon: HudIcon::None,
            primary_text: format!("Secure input on ({app}) · keys paused"),
            secondary_text: None,
            progress: None,
            dismiss_after_ms: Some(3000),
        },
        HudState::NoMicrophone => HudModel {
            accent: HudAccent::Error,
            icon: HudIcon::None,
            primary_text: "No microphone found · dictation off".into(),
            secondary_text: None,
            progress: None,
            dismiss_after_ms: Some(4000),
        },
        HudState::MicrophoneOff => HudModel {
            accent: HudAccent::Error,
            icon: HudIcon::None,
            primary_text: "Microphone access is off · open System Settings".into(),
            secondary_text: None,
            progress: None,
            dismiss_after_ms: Some(4000),
        },
        HudState::InsertBlocked => HudModel {
            accent: HudAccent::Error,
            icon: HudIcon::None,
            primary_text: "Can't insert: Accessibility is off · text in Recent".into(),
            secondary_text: None,
            progress: None,
            dismiss_after_ms: Some(4000),
        },
        HudState::ModelMissing => HudModel {
            accent: HudAccent::Error,
            icon: HudIcon::None,
            primary_text: "Model not downloaded · open Settings → Model".into(),
            secondary_text: None,
            progress: None,
            dismiss_after_ms: Some(4000),
        },
        HudState::ModelDownloading { percent } => HudModel {
            accent: HudAccent::Neutral,
            icon: HudIcon::None,
            primary_text: format!("Model downloading · {percent}%"),
            secondary_text: None,
            progress: Some((*percent).clamp(0, 100) as f32 / 100.0),
            dismiss_after_ms: None,
        },
        HudState::ModelReady => HudModel {
            accent: HudAccent::Ok,
            icon: HudIcon::None,
            primary_text: "Model ready".into(),
            secondary_text: None,
            progress: None,
            dismiss_after_ms: Some(900),
        },
    }
}

/// The optional second line of a state: the listening partial with the
/// caret (or `listening…` before the first word) plus the H16/H17 notes,
/// or the H10 confirm body. One string per drawn line (§3.3).
pub fn detail_lines(state: &HudState) -> Vec<String> {
    match state {
        HudState::ListeningDictate {
            partial,
            bluetooth,
            missing_device,
            ..
        } => {
            let mut lines = vec![match partial {
                Some(words) if !words.trim().is_empty() => format!("{words} ▍"),
                _ => "listening…".into(),
            }];
            if *bluetooth {
                lines.push("Bluetooth mic: lower quality, more latency".into());
            }
            if let Some(name) = missing_device {
                lines.push(format!("Mic \"{name}\" not found · using system input"));
            }
            lines
        }
        HudState::ListeningCommand { partial, .. } => vec![match partial {
            Some(words) if !words.trim().is_empty() => format!("{words} ▍"),
            _ => "listening…".into(),
        }],
        HudState::Confirm {
            command,
            first_step,
        } => vec![format!(
            "Run \"{command}\"? {first_step} · Return to run · Esc to cancel"
        )],
        _ => Vec::new(),
    }
}

/// Keep the tail of a long partial text: the HUD must show the newest words
/// (prototype `v3.html`: the row clips; the view truncates instead).
/// Returns the last `max_chars` characters on a char boundary.
pub fn tail_text(text: &str, max_chars: usize) -> &str {
    if max_chars == 0 {
        return "";
    }
    let count = text.chars().count();
    if count <= max_chars {
        return text;
    }
    let idx = text
        .char_indices()
        .nth(count - max_chars)
        .map(|(i, _)| i)
        .unwrap_or(0);
    &text[idx..]
}

/// How many characters of partial text the HUD keeps (§3.3 H4).
pub const PARTIAL_TAIL_CHARS: usize = 90;

/// One sample of every state, in spec order, for the gallery and eyeballing.
pub fn all_samples() -> Vec<(&'static str, HudState)> {
    vec![
        (
            "H1 listening dictate",
            HudState::ListeningDictate {
                level: 0.7,
                hands_free: false,
                loading: None,
                partial: None,
                countdown_ms: None,
                bluetooth: false,
                missing_device: None,
            },
        ),
        (
            "H1 hands-free",
            HudState::ListeningDictate {
                level: 0.5,
                hands_free: true,
                loading: None,
                partial: None,
                countdown_ms: None,
                bluetooth: false,
                missing_device: None,
            },
        ),
        (
            "H2 listening command",
            HudState::ListeningCommand {
                level: 0.6,
                loading: None,
                partial: None,
            },
        ),
        (
            "H3 model loads under speech",
            HudState::ListeningDictate {
                level: 0.4,
                hands_free: false,
                loading: Some(0.4),
                partial: None,
                countdown_ms: None,
                bluetooth: false,
                missing_device: None,
            },
        ),
        (
            "H4 live partial text",
            HudState::ListeningDictate {
                level: 0.8,
                hands_free: false,
                loading: None,
                partial: Some("the quick brown fox jumps over the lazy dog".into()),
                countdown_ms: None,
                bluetooth: false,
                missing_device: None,
            },
        ),
        (
            "H5 countdown",
            HudState::ListeningDictate {
                level: 0.5,
                hands_free: false,
                loading: None,
                partial: Some("still talking after almost two minutes".into()),
                countdown_ms: Some(15_000),
                bluetooth: false,
                missing_device: None,
            },
        ),
        (
            "H6 decoding",
            HudState::Decoding {
                mode: Mode::Dictate,
                finishing_load: false,
            },
        ),
        (
            "H6 decoding while loading",
            HudState::Decoding {
                mode: Mode::Dictate,
                finishing_load: true,
            },
        ),
        (
            "H7 inserted",
            HudState::Inserted {
                words: 9,
                latency_ms: None,
            },
        ),
        (
            "H7 inserted with latency",
            HudState::Inserted {
                words: 9,
                latency_ms: Some(142),
            },
        ),
        (
            "H8 ran command",
            HudState::RanCommand {
                label: "start working".into(),
            },
        ),
        ("H9 cancelled", HudState::Cancelled),
        (
            "H10 confirm prompt",
            HudState::Confirm {
                command: "clean build".into(),
                first_step: "cargo clean".into(),
            },
        ),
        ("H10 confirm timeout", HudState::ConfirmExpired),
        (
            "H11 no match",
            HudState::NoMatch {
                suggestion: "start working".into(),
            },
        ),
        ("H11 no command", HudState::NoCommandMatched),
        (
            "resolve failed",
            HudState::ResolveFailed {
                label: "open note".into(),
                reason: "missing slot value".into(),
            },
        ),
        (
            "H12 step failure",
            HudState::StepFailed {
                step: 2,
                total: 3,
                action: "window.tile".into(),
                reason: "no window \"terminal\"".into(),
            },
        ),
        (
            "H13 config error",
            HudState::ConfigError {
                location: "keybindings.json:12".into(),
                message: "unknown key \"hald\"".into(),
            },
        ),
        ("H14 focus lost", HudState::FocusLost),
        (
            "H15 secure input",
            HudState::SecureInput {
                app: "Terminal".into(),
            },
        ),
        (
            "H16 bluetooth mic",
            HudState::ListeningDictate {
                level: 0.5,
                hands_free: false,
                loading: None,
                partial: Some("dictating over bluetooth".into()),
                countdown_ms: None,
                bluetooth: true,
                missing_device: None,
            },
        ),
        (
            "H17 missing device",
            HudState::ListeningDictate {
                level: 0.5,
                hands_free: false,
                loading: None,
                partial: None,
                countdown_ms: None,
                bluetooth: false,
                missing_device: Some("Scarlett 2i2".into()),
            },
        ),
        ("H18 model ready", HudState::ModelReady),
        ("no microphone", HudState::NoMicrophone),
        ("microphone off", HudState::MicrophoneOff),
        ("accessibility off", HudState::InsertBlocked),
        ("model missing", HudState::ModelMissing),
        (
            "model downloading",
            HudState::ModelDownloading { percent: 42 },
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(state: &HudState) -> HudModel {
        render_model(state)
    }

    #[test]
    fn listening_dictate_copy_and_meta() {
        let m = model(&HudState::ListeningDictate {
            level: 0.7,
            hands_free: false,
            loading: None,
            partial: None,
            countdown_ms: None,
            bluetooth: false,
            missing_device: None,
        });
        assert_eq!(m.accent, HudAccent::Dictate);
        assert_eq!(m.icon, HudIcon::Level(0.7));
        assert_eq!(m.primary_text, "Dictate");
        assert_eq!(m.secondary_text.as_deref(), Some("esc to cancel"));
        assert_eq!(
            detail_lines(&HudState::ListeningDictate {
                level: 0.7,
                hands_free: false,
                loading: None,
                partial: None,
                countdown_ms: None,
                bluetooth: false,
                missing_device: None,
            }),
            vec!["listening…"]
        );
        assert_eq!((m.progress, m.dismiss_after_ms), (None, None));
    }

    #[test]
    fn hands_free_and_countdown_meta() {
        let hf = HudState::ListeningDictate {
            level: 0.5,
            hands_free: true,
            loading: None,
            partial: None,
            countdown_ms: None,
            bluetooth: false,
            missing_device: None,
        };
        assert_eq!(
            model(&hf).secondary_text.as_deref(),
            Some("hands-free · esc to cancel")
        );
        let cd = HudState::ListeningDictate {
            level: 0.5,
            hands_free: false,
            loading: None,
            partial: Some("words".into()),
            countdown_ms: Some(15_000),
            bluetooth: false,
            missing_device: None,
        };
        let m = model(&cd);
        assert_eq!(m.secondary_text.as_deref(), Some("0:15 left"));
        assert_eq!(detail_lines(&cd), vec!["words ▍"]);
    }

    #[test]
    fn listening_command_copy() {
        let m = model(&HudState::ListeningCommand {
            level: 1.5,
            loading: Some(0.4),
            partial: None,
        });
        assert_eq!(m.accent, HudAccent::Command);
        assert_eq!(m.icon, HudIcon::Level(1.0));
        assert_eq!(m.primary_text, "Command");
        assert_eq!(m.progress, Some(0.4));
    }

    #[test]
    fn listening_notes_append_under_the_partial() {
        let state = HudState::ListeningDictate {
            level: 0.5,
            hands_free: false,
            loading: None,
            partial: Some("hi".into()),
            countdown_ms: None,
            bluetooth: true,
            missing_device: Some("Scarlett 2i2".into()),
        };
        assert_eq!(
            detail_lines(&state),
            vec![
                "hi ▍",
                "Bluetooth mic: lower quality, more latency",
                "Mic \"Scarlett 2i2\" not found · using system input",
            ]
        );
        assert_eq!(detail_lines(&HudState::Cancelled), Vec::<String>::new());
    }

    #[test]
    fn decoding_copy() {
        let m = model(&HudState::Decoding {
            mode: Mode::Dictate,
            finishing_load: false,
        });
        assert_eq!(m.icon, HudIcon::Spinner);
        assert_eq!(m.secondary_text.as_deref(), Some("decoding"));
        let m = model(&HudState::Decoding {
            mode: Mode::Command,
            finishing_load: true,
        });
        assert_eq!(m.primary_text, "Command");
        assert_eq!(m.secondary_text.as_deref(), Some("finishing model load"));
    }

    #[test]
    fn inserted_copy_and_latency() {
        let m = model(&HudState::Inserted {
            words: 9,
            latency_ms: None,
        });
        assert_eq!(m.primary_text, "Inserted 9 words");
        assert_eq!(m.secondary_text, None);
        assert_eq!(m.dismiss_after_ms, Some(1200));
        let m = model(&HudState::Inserted {
            words: 1,
            latency_ms: Some(142),
        });
        assert_eq!(m.primary_text, "Inserted 1 word");
        assert_eq!(m.secondary_text.as_deref(), Some("142 ms"));
    }

    #[test]
    fn ran_cancelled_confirm_copies() {
        assert_eq!(
            model(&HudState::RanCommand {
                label: "start working".into()
            })
            .primary_text,
            "Ran \"start working\""
        );
        assert_eq!(
            model(&HudState::RanCommand { label: "x".into() }).dismiss_after_ms,
            Some(1300)
        );
        assert_eq!(model(&HudState::Cancelled).primary_text, "Cancelled");
        assert_eq!(model(&HudState::Cancelled).dismiss_after_ms, Some(700));
        let m = model(&HudState::Confirm {
            command: "clean build".into(),
            first_step: "cargo clean".into(),
        });
        assert_eq!(m.primary_text, "Command");
        assert_eq!(
            detail_lines(&HudState::Confirm {
                command: "clean build".into(),
                first_step: "cargo clean".into(),
            }),
            vec!["Run \"clean build\"? cargo clean · Return to run · Esc to cancel"]
        );
        assert_eq!(model(&HudState::ConfirmExpired).primary_text, "Not run");
        assert_eq!(
            model(&HudState::ConfirmExpired).dismiss_after_ms,
            Some(1000)
        );
    }

    #[test]
    fn runner_no_match_and_failed_copies() {
        let m = model(&HudState::NoCommandMatched);
        assert_eq!(m.primary_text, "No command matched");
        assert_eq!(m.dismiss_after_ms, Some(2500));
        assert!(detail_lines(&HudState::NoCommandMatched).is_empty());
        let failed = HudState::ResolveFailed {
            label: "open note".into(),
            reason: "missing slot value".into(),
        };
        let m = model(&failed);
        assert_eq!(
            m.primary_text,
            "Could not run \"open note\": missing slot value"
        );
        assert_eq!(m.dismiss_after_ms, Some(4000));
        assert!(detail_lines(&failed).is_empty());
    }

    #[test]
    fn no_match_step_failure_copies() {
        let m = model(&HudState::NoMatch {
            suggestion: "start working".into(),
        });
        assert_eq!(m.primary_text, "Did you mean \"start working\"?");
        assert_eq!(m.dismiss_after_ms, Some(2500));
        let m = model(&HudState::StepFailed {
            step: 2,
            total: 3,
            action: "window.tile".into(),
            reason: "no window \"terminal\"".into(),
        });
        assert_eq!(m.accent, HudAccent::Error);
        assert_eq!(
            m.primary_text,
            "Step 2 of 3 failed: window.tile · no window \"terminal\""
        );
        assert_eq!(m.dismiss_after_ms, Some(4000));
    }

    #[test]
    fn config_focus_secure_copies() {
        let m = model(&HudState::ConfigError {
            location: "keybindings.json:12".into(),
            message: "unknown key \"hald\"".into(),
        });
        assert_eq!(
            m.primary_text,
            "keybindings.json:12: unknown key \"hald\" · using last good config"
        );
        assert_eq!(m.dismiss_after_ms, Some(6000));
        assert_eq!(
            model(&HudState::FocusLost).primary_text,
            "Focus lost: text in Recent"
        );
        assert_eq!(
            model(&HudState::SecureInput {
                app: "Terminal".into()
            })
            .primary_text,
            "Secure input on (Terminal) · keys paused"
        );
        assert_eq!(
            model(&HudState::SecureInput { app: "x".into() }).dismiss_after_ms,
            Some(3000)
        );
    }

    #[test]
    fn error_states_copy_and_dismiss() {
        for (state, copy, dismiss) in [
            (
                HudState::NoMicrophone,
                "No microphone found · dictation off",
                Some(4000),
            ),
            (
                HudState::MicrophoneOff,
                "Microphone access is off · open System Settings",
                Some(4000),
            ),
            (
                HudState::InsertBlocked,
                "Can't insert: Accessibility is off · text in Recent",
                Some(4000),
            ),
            (
                HudState::ModelMissing,
                "Model not downloaded · open Settings → Model",
                Some(4000),
            ),
            (HudState::ModelReady, "Model ready", Some(900)),
        ] {
            let m = model(&state);
            assert_eq!(m.primary_text, copy);
            assert_eq!(m.dismiss_after_ms, dismiss, "{copy}");
        }
        let m = model(&HudState::ModelDownloading { percent: 42 });
        assert_eq!(m.primary_text, "Model downloading · 42%");
        assert_eq!(m.progress, Some(0.42));
        assert_eq!(m.dismiss_after_ms, None);
    }

    #[test]
    fn tail_text_keeps_the_tail() {
        assert_eq!(tail_text("hello", 90), "hello");
        assert_eq!(tail_text("hello world", 5), "world");
        assert_eq!(tail_text("héllo wörld", 5), "wörld");
        assert_eq!(tail_text("", 5), "");
        assert_eq!(tail_text("abc", 0), "");
    }

    #[test]
    fn every_sample_renders() {
        // The gallery shows every sample: each one must map without panicking.
        let samples = all_samples();
        assert!(samples.len() >= 27, "one sample per HUD state");
        for (label, state) in &samples {
            let m = render_model(state);
            assert!(!m.primary_text.is_empty(), "{label}");
        }
    }
}
