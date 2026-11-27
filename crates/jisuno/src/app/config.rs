//! Config files: first-run embeds, read helper, poll/note/restart/secure.
//!
//! Split from `app.rs` with no behavior change (step 1).

use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use gpui::App;
use jisuno_logic::hold::{CapturedKey, HoldMachine, Input, Timing};
use jisuno_logic::keybindings::{self, Keybindings};
use jisuno_logic::settings::{ConfigDir, Settings};
use jisuno_logic::voice_control::{self, Config};
use jisuno_platform::{
    keys::{Captured, KeyTap, ShouldCapture},
    permissions::{self, PermissionState},
    secure_input,
};
use jisuno_ui::hud_model::HudState as UiHud;

use super::model::{model_id, ModelState};
use super::state::{AppState, SECURE_POLL_MS};
use super::tap_config_for;

/// Dictation-limit change seen while a session is live: applied when the
/// machine is back to Idle (never mid-hold). 0 = none (the valid range is
/// 10..=600, so 0 is never a real limit). Process-global: one app instance
/// owns the mic, and `AppState` gains no field for it (see mod.rs).
static PENDING_DICTATION_LIMIT_S: AtomicU64 = AtomicU64::new(0);

/// The event tap is down (no tap yet, or a restart failed): the retry
/// schedule and whether the user has been told. `None` while the tap runs.
/// Process-global for the same reason.
static TAP_DOWN: Mutex<Option<TapDown>> = Mutex::new(None);

/// Welcome was opened for a tap outage this launch; later outages show the
/// keys-paused HUD instead.
static TAP_WELCOME_SHOWN: AtomicBool = AtomicBool::new(false);

/// Retry cadence while the event tap is down.
pub(crate) const TAP_RETRY: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TapDown {
    next_try: Instant,
    announced: bool,
}

/// Model switch seen while busy: the new model id, applied once idle.
static PENDING_MODEL: OnceLock<Mutex<Option<String>>> = OnceLock::new();

/// What a model change does: switch now, defer until idle, or just rename
/// (nothing loaded yet — the next hold loads on demand).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelSwitch {
    Now,
    Defer,
    RenameOnly,
}

/// Pure decision for [`ModelSwitch`]: switch while idle and something is
/// loaded/loading, defer while busy, rename when unloaded.
pub(crate) fn model_switch_action(state: &ModelState, busy: bool) -> ModelSwitch {
    if busy {
        return ModelSwitch::Defer;
    }
    match state {
        ModelState::Warm | ModelState::Loading { .. } => ModelSwitch::Now,
        ModelState::Unloaded | ModelState::Failed => ModelSwitch::RenameOnly,
    }
}

fn with_tap_down<R>(f: impl FnOnce(&mut Option<TapDown>) -> R) -> R {
    f(&mut TAP_DOWN.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Whether the event tap is down (keys paused, badge shown).
pub(crate) fn tap_is_down() -> bool {
    with_tap_down(|down| down.is_some())
}

/// Start an outage: the first failure schedules a retry one interval out.
/// A second failure within the same outage keeps the schedule.
pub(crate) fn mark_tap_down(now: Instant) {
    with_tap_down(|down| {
        if down.is_none() {
            *down = Some(TapDown {
                next_try: now + TAP_RETRY,
                announced: false,
            });
        }
    });
}

/// End the outage; true if the tap was down.
fn clear_tap_down() -> bool {
    with_tap_down(|down| down.take().is_some())
}

/// Whether a tap retry is due at `now`. Pure: unit-tested below.
pub(crate) fn tap_retry_due(next_try: Instant, now: Instant) -> bool {
    now >= next_try
}

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

/// The log file rotates to `jisuno.log.1` above this size: at startup, and
/// while running (`poll_log_size`).
const LOG_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// How often the running app checks the log size.
const LOG_CHECK_EVERY: Duration = Duration::from_secs(600);

const LOG_FILE: &str = "jisuno.log";

/// The log file stderr was redirected to, set once by `redirect_stderr_to_log`.
static LOG_PATH: OnceLock<std::path::PathBuf> = OnceLock::new();

/// `~/Library/Logs/Jisuno` under `home`. Pure: unit-tested below.
fn log_dir(home: &std::path::Path) -> std::path::PathBuf {
    home.join("Library").join("Logs").join("Jisuno")
}

/// Launched without a terminal (Finder, `open`, login item): send stderr to
/// `~/Library/Logs/Jisuno/jisuno.log`, appending. A log over 5 MB moves to
/// `jisuno.log.1` first. A terminal keeps stderr as it is. Best effort: a
/// failure here leaves stderr where it was.
pub(crate) fn redirect_stderr_to_log() {
    use std::os::unix::io::AsRawFd;

    // SAFETY: a pure query on fd 2.
    if unsafe { libc::isatty(libc::STDERR_FILENO) } == 1 {
        return;
    }
    let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) else {
        return;
    };
    let dir = log_dir(std::path::Path::new(&home));
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(LOG_FILE);
    let too_big = std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_MAX_BYTES);
    if too_big {
        let _ = std::fs::rename(&path, dir.join("jisuno.log.1"));
    }
    let Some(file) = open_log(&path) else {
        return;
    };
    // SAFETY: both descriptors are open; dup2 copies the file into fd 2, and
    // `file` can close afterwards.
    unsafe {
        libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO);
    }
    let _ = LOG_PATH.set(path);
}

fn open_log(path: &std::path::Path) -> Option<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .ok()
}

/// Rotate while running: over `LOG_MAX_BYTES`, the file moves to
/// `jisuno.log.1` and a new `jisuno.log` is opened on fd 2. No-op when stderr
/// is not the log file (a terminal, or a failed redirect).
pub(crate) fn rotate_log_if_big() {
    use std::os::unix::io::AsRawFd;

    let Some(path) = LOG_PATH.get() else {
        return;
    };
    if !std::fs::metadata(path).is_ok_and(|m| m.len() > LOG_MAX_BYTES) {
        return;
    }
    let Some(dir) = path.parent() else {
        return;
    };
    if std::fs::rename(path, dir.join("jisuno.log.1")).is_err() {
        return;
    }
    let Some(file) = open_log(path) else {
        return;
    };
    // SAFETY: as in `redirect_stderr_to_log`; the old fd 2 target is closed
    // by dup2.
    unsafe {
        libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO);
    }
}

// --- Polls ---

impl AppState {
    // --- Polls ---

    /// Size check for the log, every `LOG_CHECK_EVERY`: one metadata call.
    pub(crate) fn poll_log_size(&mut self) {
        if self.log_checked_at.elapsed() < LOG_CHECK_EVERY {
            return;
        }
        self.log_checked_at = Instant::now();
        rotate_log_if_big();
    }

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
        let login_item = (fresh.login_item != self.settings.login_item).then_some(fresh.login_item);
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
            let new_model = model_id(&fresh);
            if new_model != self.model_name {
                self.model_name.clone_from(&new_model);
                match model_switch_action(&self.model_state, !self.can_unload()) {
                    ModelSwitch::Now => self.switch_model(cx, "model switch"),
                    ModelSwitch::Defer => {
                        if let Ok(mut pending) =
                            PENDING_MODEL.get_or_init(|| Mutex::new(None)).lock()
                        {
                            *pending = Some(new_model);
                        }
                        self.log("model switch deferred until idle");
                    }
                    ModelSwitch::RenameOnly => {}
                }
            }
            self.settings = fresh;
        }
        self.last_config_error = None;
        if let Some(enabled) = login_item {
            self.apply_login_item(cx, enabled);
        }
    }

    /// Register or unregister the login item. Unbundled runs only log: the
    /// registration needs the signed app. Any other failure shows once.
    pub(crate) fn apply_login_item(&mut self, cx: &mut App, enabled: bool) {
        if !jisuno_platform::login_item::in_app_bundle() {
            self.log("login item: needs the bundled app");
            return;
        }
        if let Err(e) = jisuno_platform::login_item::set_enabled(enabled) {
            self.note_config_error(cx, &format!("settings.json: login item failed: {e:#}"));
        }
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

    /// Apply a deferred model switch once idle. A newer reload supersedes
    /// the pending name; if the model already unloaded meanwhile, the
    /// rename stands and the next hold loads on demand.
    pub(crate) fn apply_pending_model(&mut self, cx: &mut App) {
        let pending = PENDING_MODEL
            .get_or_init(|| Mutex::new(None))
            .lock()
            .map(|mut slot| slot.take())
            .unwrap_or(None);
        let Some(name) = pending else {
            return;
        };
        if name != self.model_name {
            return;
        }
        if !self.can_unload() {
            if let Ok(mut slot) = PENDING_MODEL.get_or_init(|| Mutex::new(None)).lock() {
                *slot = Some(name);
            }
            return;
        }
        if matches!(
            self.model_state,
            ModelState::Warm | ModelState::Loading { .. }
        ) {
            self.switch_model(cx, "model switch");
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
            permissions::input_monitoring() == PermissionState::Granted && !tap_is_down(),
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

    /// Start the event tap with the current bindings and capture mode, then
    /// replace the old one. The old tap stays up when the new one fails.
    pub(crate) fn start_tap(&mut self) -> Result<()> {
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
        let (tap, rx) = KeyTap::start(
            tap_config_for(&self.bindings, self.key_capturing.is_some()),
            should_capture,
        )?;
        if let Some(old) = self.tap.replace(tap) {
            if let Err(e) = old.shutdown() {
                self.log(&format!("tap shutdown: {e:#}"));
            }
        }
        self.tap_rx = Some(rx);
        Ok(())
    }

    pub(crate) fn restart_tap(&mut self) {
        match self.start_tap() {
            Ok(()) => {
                if clear_tap_down() {
                    self.perm_granted[2] = true;
                }
                self.log("tap restarted");
            }
            Err(e) => {
                // The old tap stays up; the tick retries until a tap starts.
                self.log(&format!("tap restart failed: {e:#}"));
                mark_tap_down(Instant::now());
            }
        }
    }

    /// Retry the event tap while it is down. The first tick of an outage
    /// requests Input Monitoring once and shows Welcome (or the keys-paused
    /// HUD when Welcome was already shown). Each retry checks the permission
    /// first, so no tap is created until it is granted.
    pub(crate) fn poll_tap_retry(&mut self, cx: &mut App) {
        let Some(down) = with_tap_down(|down| *down) else {
            return;
        };
        if !down.announced {
            with_tap_down(|down| {
                if let Some(down) = down {
                    down.announced = true;
                }
            });
            self.announce_tap_down(cx);
        }
        let now = Instant::now();
        if !tap_retry_due(down.next_try, now) {
            return;
        }
        with_tap_down(|down| {
            if let Some(down) = down {
                down.next_try = now + TAP_RETRY;
            }
        });
        if permissions::input_monitoring() != PermissionState::Granted {
            return;
        }
        match self.start_tap() {
            Ok(()) => {
                clear_tap_down();
                self.perm_granted[2] = true;
                self.log("event tap started");
            }
            Err(e) => self.log(&format!("event tap retry failed: {e:#}")),
        }
    }

    fn announce_tap_down(&mut self, cx: &mut App) {
        self.log("event tap down: keys paused until Input Monitoring is granted");
        self.perm_granted[2] = false;
        if let Err(e) = permissions::request_input_monitoring() {
            self.log(&format!("input monitoring request failed: {e:#}"));
        }
        // An open Welcome shows the permission rows live: nothing more to say.
        if self.welcome_win.is_some() {
            return;
        }
        if !TAP_WELCOME_SHOWN.swap(true, Ordering::SeqCst) {
            self.open_welcome(cx);
        } else {
            self.show_ephemeral(
                cx,
                UiHud::ConfigError {
                    location: "Keys paused".to_string(),
                    message: "Input Monitoring is off".to_string(),
                },
                "tap down",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{pending_timing_take, PENDING_DICTATION_LIMIT_S};
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    #[test]
    fn log_dir_is_under_library_logs() {
        assert_eq!(
            super::log_dir(std::path::Path::new("/Users/x")),
            std::path::PathBuf::from("/Users/x/Library/Logs/Jisuno")
        );
    }

    /// The tap retry waits the full interval, then is due at and after it.
    #[test]
    fn tap_retry_is_due_after_interval() {
        use super::{tap_retry_due, TAP_RETRY};
        let now = Instant::now();
        let next = now + TAP_RETRY;
        assert!(!tap_retry_due(next, now));
        assert!(!tap_retry_due(next, now + Duration::from_millis(1999)));
        assert!(tap_retry_due(next, next));
        assert!(tap_retry_due(next, next + Duration::from_secs(5)));
    }

    /// One outage: the first failure schedules, a second keeps the schedule,
    /// and clearing reports whether a tap was down.
    #[test]
    fn tap_outage_marks_and_clears() {
        use super::{clear_tap_down, mark_tap_down, tap_is_down, with_tap_down, TAP_RETRY};
        let now = Instant::now();
        assert!(!clear_tap_down());
        mark_tap_down(now);
        mark_tap_down(now + Duration::from_secs(1));
        assert!(tap_is_down());
        assert_eq!(
            with_tap_down(|down| down.map(|d| d.next_try)),
            Some(now + TAP_RETRY)
        );
        assert!(clear_tap_down());
        assert!(!tap_is_down());
    }

    /// A model change switches at once while idle and loaded, waits while
    /// busy, and renames only when nothing is loaded.
    #[test]
    fn model_switch_decision() {
        use super::model_switch_action;
        use super::ModelState;
        use super::ModelSwitch;
        assert_eq!(
            model_switch_action(&ModelState::Warm, false),
            ModelSwitch::Now
        );
        assert_eq!(
            model_switch_action(&ModelState::Loading { gen: 1 }, false),
            ModelSwitch::Now
        );
        assert_eq!(
            model_switch_action(&ModelState::Warm, true),
            ModelSwitch::Defer
        );
        assert_eq!(
            model_switch_action(&ModelState::Unloaded, false),
            ModelSwitch::RenameOnly
        );
        assert_eq!(
            model_switch_action(&ModelState::Failed, false),
            ModelSwitch::RenameOnly
        );
    }

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
