//! Shared app state: the `AppState` struct plus the small value types the
//! pipeline, HUD, model, config and status modules pass around.
//!
//! Split from `app.rs` with no behavior change (step 1): this file owns the
//! data, the other modules own `impl AppState` method groups.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{atomic::AtomicBool, mpsc, Arc, Mutex};
use std::time::Instant;

use gpui::{AnyWindowHandle, App};
use jisuno_audio::mic::Capture;
use jisuno_logic::hold::{CapturedKey, HoldMachine, Input, Mode};
use jisuno_logic::keybindings::Keybindings;
use jisuno_logic::runner::Plan;
use jisuno_logic::settings::{LastGood, Settings};
use jisuno_logic::voice_control::Config;
use jisuno_platform::{
    frontmost::AppInfo,
    keys::{KeyEvent, KeyTap},
    status_item::{MenuAction, StatusItem},
};
use jisuno_ui::hud_model::HudState as UiHud;
use jisuno_ui::tokens::ThemeJson;

use super::command::{CommandDone, HookRequest};
use super::model::ModelState;
use super::model::{ActiveDownload, DownloadEvent};
use super::socket::SocketSnapshot;
use super::windows::{BenchEvent, BenchValues, PopoverHandle, SettingsHandle, WelcomeHandle};
use super::worker::{FromWorker, ToWorker};
use super::DebugOpen;
use jisuno_logic::keybindings::{Command as KeyCommand, When as KeyWhen};
use jisuno_ui::popover_model::PopoverIntent;
use jisuno_ui::settings_model::SettingsIntent;
use jisuno_ui::welcome_model::WelcomeIntent;

/// Last-good holder with show-once errors, keyed by message. `LastGood` in
/// `jisuno-logic` fixes its error type to the settings diagnostic, so the
/// startup path (keybindings, theme) uses this instead; hot reload uses
/// `LastGood` with converted diagnostics.
pub(crate) struct Holder<T> {
    pub(crate) current: T,
    pub(crate) last_error: Option<String>,
}

impl<T> Holder<T> {
    pub(crate) fn set(&mut self, value: T) {
        self.current = value;
        self.last_error = None;
    }

    /// Record an error; true when it is new (the HUD shows it once).
    pub(crate) fn err(&mut self, message: String) -> bool {
        if self.last_error.as_deref() == Some(message.as_str()) {
            return false;
        }
        self.last_error = Some(message);
        true
    }
}

pub(crate) struct PendingTranscribe {
    pub(crate) id: u64,
    pub(crate) mode: Mode,
    pub(crate) key_up: Instant,
    /// App focused at key-down, captured when the machine accepted the
    /// press: the utterance inserts here even if focus moved by key-up.
    /// `None` means focus was already lost at key-down.
    pub(crate) target: Option<AppInfo>,
}

pub(crate) struct HudWindow {
    pub(crate) handle: AnyWindowHandle,
    pub(crate) dismiss_at: Option<Instant>,
    pub(crate) sticky: bool,
}

pub(crate) struct ListeningUi {
    pub(crate) dictate: bool,
    pub(crate) hands_free: bool,
    pub(crate) level: f32,
    pub(crate) loading_since: Option<Instant>,
    pub(crate) countdown_ms: Option<u64>,
    /// Latest live partial text (H4), `None` before the first words or when
    /// `settings.partials` is off.
    pub(crate) partial: Option<String>,
}

// Read by the popover (comes later); until then entries are logged on push.
#[allow(dead_code)]
pub(crate) struct RecentEntry {
    pub(crate) text: String,
    pub(crate) tag: Option<String>,
}

pub(crate) const TICK_MS: u64 = 20;
pub(crate) const SECURE_POLL_MS: u64 = 1000;

pub(crate) struct AppState {
    pub(crate) start: Instant,
    pub(crate) machine: Arc<Mutex<HoldMachine>>,
    pub(crate) bindings: Keybindings,
    pub(crate) settings: Settings,
    pub(crate) last_secure_poll: Instant,
    pub(crate) pending: Option<PendingTranscribe>,
    pub(crate) capture: Option<Capture>,
    pub(crate) mic_open: bool,
    pub(crate) mic_rate: u32,
    /// 16 kHz mono samples accumulated while the stream is not begun (the
    /// cold path decodes this buffer when the load lands).
    pub(crate) buffer: Vec<f32>,
    pub(crate) heard_speech: bool,
    pub(crate) handsfree_vad: Option<jisuno_logic::vad::Vad>,
    pub(crate) level: f32,
    pub(crate) hud: Option<HudWindow>,
    pub(crate) hud_queue: VecDeque<UiHud>,
    pub(crate) listening: Option<ListeningUi>,
    pub(crate) show_latency: bool,
    pub(crate) handsfree_armed: bool,
    pub(crate) perm_granted: [bool; 3],
    pub(crate) model_present: bool,
    pub(crate) model_state: ModelState,
    pub(crate) model_gen: u64,
    pub(crate) model_name: String,
    /// Streaming utterance against the helper child: `Some` while the mic
    /// is open (or the key-up transcribe is queued). Ids are app-local
    /// (0 is reserved for the worker one-shot path); the machine's
    /// transcribe id travels separately in `ToWorker::Finish.reply`.
    pub(crate) stream: Option<u64>,
    pub(crate) stream_seq: u64,
    /// True once `Begin` went out (warm at mic open, or flushed on Warm).
    /// False while cold: audio only accumulates in `buffer`.
    pub(crate) stream_begun: bool,
    /// A `PartialReq` is waiting for its reply (one at a time).
    pub(crate) partial_in_flight: bool,
    pub(crate) last_partial_poll: Instant,
    /// Latest partial text, tail-truncated for the HUD (H4).
    pub(crate) partial_text: Option<String>,
    /// Incremental 48 kHz -> 16 kHz state for the mic drain (keeps filter
    /// context across chunks; reset on every open).
    pub(crate) resampler: Option<jisuno_audio::audio::StreamingResampler>,
    /// Debug-WAV-as-mic: the whole clip at 16 kHz, fed in 100 ms chunks.
    pub(crate) debug_pcm: Option<Vec<f32>>,
    pub(crate) debug_pos: usize,
    pub(crate) debug_next_at: Option<Instant>,
    /// Helper child RSS in MiB (from the last Warm/Transcript STATUS).
    pub(crate) child_rss_mb: Option<u64>,
    /// The crash HUD has been shown since the last successful Warm.
    pub(crate) worker_crash_shown: bool,
    pub(crate) last_active: Instant,
    pub(crate) target: Option<AppInfo>,
    pub(crate) to_worker: mpsc::Sender<ToWorker>,
    pub(crate) from_worker: mpsc::Receiver<FromWorker>,
    pub(crate) tap_rx: Option<mpsc::Receiver<KeyEvent>>,
    pub(crate) tap: Option<KeyTap>,
    pub(crate) menu_rx: mpsc::Receiver<MenuAction>,
    pub(crate) status: StatusItem,
    pub(crate) download: Option<ActiveDownload>,
    pub(crate) download_tx: mpsc::Sender<DownloadEvent>,
    pub(crate) download_rx: mpsc::Receiver<DownloadEvent>,
    /// Keybinding recording: the Settings Keybindings pane is waiting for
    pub(crate) debug_open: Option<DebugOpen>,
    /// Keybinding recording: the Settings Keybindings pane is waiting for
    /// the next chord/hold (Esc cancels). While set, the tap runs in
    /// capture mode and forwards keys to `feed_recorded_key`.
    pub(crate) key_capturing: Option<(KeyCommand, Option<KeyWhen>)>,
    /// The tap thread swallows chords/keys while this is set (recording).
    /// Shared with the `ShouldCapture` closure, which cannot borrow state.
    pub(crate) tap_capture: Arc<AtomicBool>,
    pub(crate) settings_win: Option<SettingsHandle>,
    pub(crate) settings_tx: mpsc::Sender<SettingsIntent>,
    pub(crate) settings_rx: mpsc::Receiver<SettingsIntent>,
    pub(crate) welcome_win: Option<WelcomeHandle>,
    pub(crate) welcome_tx: mpsc::Sender<WelcomeIntent>,
    pub(crate) welcome_rx: mpsc::Receiver<WelcomeIntent>,
    /// Last bench numbers (cache file or a finished run) and run state.
    pub(crate) bench_report: Option<BenchValues>,
    pub(crate) bench_error: Option<String>,
    pub(crate) bench_running: bool,
    pub(crate) bench_tx: mpsc::Sender<BenchEvent>,
    pub(crate) bench_rx: mpsc::Receiver<BenchEvent>,
    pub(crate) popover: Option<PopoverHandle>,
    pub(crate) popover_toast: Option<(String, Instant)>,
    pub(crate) popover_tx: mpsc::Sender<PopoverIntent>,
    pub(crate) popover_rx: mpsc::Receiver<PopoverIntent>,
    pub(crate) model_line: String,
    pub(crate) perm_line: [String; 3],
    pub(crate) recent: VecDeque<RecentEntry>,
    pub(crate) announced_secure: Vec<String>,
    pub(crate) announced_bluetooth: bool,
    pub(crate) mic_satisfied: bool,
    pub(crate) bluetooth_now: bool,
    pub(crate) missing_device_note: Option<String>,
    pub(crate) secure_now: bool,
    pub(crate) last_config_error: Option<String>,
    pub(crate) debug_wav: Option<PathBuf>,
    pub(crate) debug_hold: Option<(Instant, Instant, bool)>,
    pub(crate) quit_requested: bool,
    pub(crate) voice_config: Config,
    pub(crate) installed_apps: Vec<String>,
    pub(crate) installed_at: Instant,
    pub(crate) pending_plan: Option<Plan>,
    pub(crate) confirming: Option<Plan>,
    pub(crate) esc_cancel: bool,
    pub(crate) cmd_tx: mpsc::Sender<CommandDone>,
    pub(crate) cmd_rx: mpsc::Receiver<CommandDone>,
    pub(crate) hook_tx: mpsc::Sender<HookRequest>,
    pub(crate) hook_rx: mpsc::Receiver<HookRequest>,
    pub(crate) settings_good: LastGood<Settings>,
    pub(crate) bindings_good: LastGood<Keybindings>,
    pub(crate) voice_good: LastGood<Config>,
    pub(crate) theme_good: LastGood<ThemeJson>,
    pub(crate) watch_rx: mpsc::Receiver<PathBuf>,
    pub(crate) socket_snapshot: Arc<Mutex<SocketSnapshot>>,
}

impl AppState {
    pub(crate) fn log(&self, msg: &str) {
        let e = self.start.elapsed();
        eprintln!(
            "[{:02}:{:02}.{:03}] {msg}",
            e.as_secs() / 60,
            e.as_secs() % 60,
            e.subsec_millis()
        );
    }

    pub(crate) fn input(&mut self, cx: &mut App, ev: Input, tag: &str) {
        let now = self.now_ms();
        let effects = self
            .machine
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .handle(ev, now);
        self.log(&format!("hold {tag} -> {} effect(s)", effects.len()));
        self.exec_effects(cx, effects);
    }

    pub(crate) fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    pub(crate) fn machine_captures(&self, key: CapturedKey) -> bool {
        self.machine
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .captures(key)
    }
}

/// Full dictated/partial/command text reaches the logs only when
/// `JISUNO_LOG_TEXT=1`; by default logs carry lengths/word counts only.
pub(crate) fn log_full_text() -> bool {
    std::env::var_os("JISUNO_LOG_TEXT").is_some_and(|v| v == "1")
}

/// Log-safe rendering of user speech: the full text with
/// `JISUNO_LOG_TEXT=1`, otherwise `N chars, M words`.
pub(crate) fn text_summary(text: &str) -> String {
    if log_full_text() {
        format!("{text:?}")
    } else {
        format!(
            "{} chars, {} words",
            text.chars().count(),
            text.split_whitespace().count()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{log_full_text, text_summary};

    #[test]
    fn speech_stays_out_of_logs_by_default() {
        let prev = std::env::var_os("JISUNO_LOG_TEXT");
        std::env::remove_var("JISUNO_LOG_TEXT");
        assert!(!log_full_text());
        assert_eq!(text_summary("hello brave world"), "17 chars, 3 words");
        assert_eq!(text_summary(""), "0 chars, 0 words");
        std::env::set_var("JISUNO_LOG_TEXT", "1");
        assert!(log_full_text());
        assert_eq!(text_summary("hello brave world"), "\"hello brave world\"");
        match prev {
            Some(v) => std::env::set_var("JISUNO_LOG_TEXT", v),
            None => std::env::remove_var("JISUNO_LOG_TEXT"),
        }
    }
}
