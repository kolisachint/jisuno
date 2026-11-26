//! Shared app state: the `AppState` struct plus the small value types the
//! pipeline, HUD, model, config and status modules pass around.
//!
//! Split from `app.rs` with no behavior change (step 1): this file owns the
//! data, the other modules own `impl AppState` method groups.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
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
use super::socket::SocketSnapshot;
use super::worker::{FromWorker, ToWorker};

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
    pub(crate) last_active: Instant,
    pub(crate) target: Option<AppInfo>,
    pub(crate) to_worker: mpsc::Sender<ToWorker>,
    pub(crate) from_worker: mpsc::Receiver<FromWorker>,
    pub(crate) tap_rx: Option<mpsc::Receiver<KeyEvent>>,
    pub(crate) tap: Option<KeyTap>,
    pub(crate) menu_rx: mpsc::Receiver<MenuAction>,
    pub(crate) status: StatusItem,
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
