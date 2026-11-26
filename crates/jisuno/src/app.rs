//! The Jisuno menu bar app: dictation MVP.
//!
//! GPUI is the app runtime (as `jisuno-ui` does): it owns the run loop and
//! the HUD windows. `gpui-pre` 0.3 has no status-item API, so the
//! `NSStatusItem` is created alongside it with objc2 on the main thread,
//! inside the GPUI run closure; menu actions travel back over a channel.
//!
//! One driver owns everything: a 20 ms GPUI timer (`tick`) drains the event
//! tap, the mic, the model worker and the menu, feeds the pure
//! [`HoldMachine`][jisuno_logic::hold] and executes its [`Effect`][];
//! transcription itself runs on a worker thread over full clips
//! (`FullClipDecode`, swappable for the streaming path being built
//! separately). Logging goes to stderr with timestamps; every hold
//! transition and the key-up → text latency are logged so the owner sees
//! real numbers.
//!
//! Standing gaps (dictation MVP): no tones, no live partial text (the
//! full-clip path has nothing to stream yet), no Settings/Welcome/popover
//! windows, command mode shows a "coming soon" chip, named input devices
//! are detected but capture still uses the system default.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context as _, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use gpui::{ActivationPolicy, App, AsyncApp};
use jisuno_asr::setup::Model as AsrModel;
use jisuno_asr::transcribe::Transcriber;
use jisuno_audio::{audio, mic::Capture};
use jisuno_logic::hold::{CapturedKey, Effect, HoldKind, HoldMachine, Input, Mode, Timing};
use jisuno_logic::keybindings::{self, Command, Key, Keybindings};
use jisuno_logic::settings::{self, ConfigDir, EmbeddedFile, Settings};
use jisuno_platform::{
    frontmost::{self, AppInfo},
    hud_placement::{hud_origin, mouse_position_gpui, HudSize, Screen},
    insert::{self, FocusLost, InsertMode},
    keys::{Captured, KeyEvent, KeyTap, RightKey, ShouldCapture, TapConfig},
    permissions::{self, Pane, PermissionState},
    secure_input,
    status_item::{IconState, MenuAction, StatusItem},
};
use jisuno_ui::hud_model::{HudState as UiHud, Mode as UiMode};
use jisuno_ui::hud_window::{
    close_hud_window, open_hud_window, update_hud_window, HUD_WINDOW_SIZE,
};

/// Launch options for the menu bar app.
#[derive(Debug, Clone, Default)]
pub struct AppOptions {
    /// Substitute this WAV file for the microphone (hidden debug flag).
    pub debug_wav: Option<PathBuf>,
    /// Simulate one internal dictate hold of this many ms, starting ~1 s
    /// after launch (hidden debug flag; bypasses the event tap).
    pub debug_hold_ms: Option<u64>,
}

impl AppOptions {
    pub fn from_env() -> Self {
        Self {
            debug_wav: std::env::var("JISUNO_DEBUG_WAV").ok().map(PathBuf::from),
            debug_hold_ms: std::env::var("JISUNO_DEBUG_HOLD_MS")
                .ok()
                .and_then(|s| s.parse().ok()),
        }
    }
}

/// Last-good holder with show-once errors, keyed by message. `LastGood` in
/// `jisuno-logic` fixes its error type to the settings diagnostic, so config
/// types with their own diagnostics (keybindings, theme) need this instead.
struct Holder<T> {
    current: T,
    last_error: Option<String>,
}

impl<T> Holder<T> {
    fn set(&mut self, value: T) {
        self.current = value;
        self.last_error = None;
    }

    /// Record an error; true when it is new (the HUD shows it once).
    fn err(&mut self, message: String) -> bool {
        if self.last_error.as_deref() == Some(message.as_str()) {
            return false;
        }
        self.last_error = Some(message);
        true
    }
}

/// Full-clip decode behind one method, so the streaming path being built
/// separately can replace this without touching the app wiring.
trait FullClipDecode {
    fn ensure_loaded(&mut self, model: AsrModel) -> Result<()>;
    fn decode_clip(&mut self, pcm_16k: &[f32]) -> Result<String>;
    fn unload(&mut self);
}

struct ParakeetEngine {
    inner: Option<Box<dyn Transcriber>>,
}

impl FullClipDecode for ParakeetEngine {
    fn ensure_loaded(&mut self, model: AsrModel) -> Result<()> {
        if self.inner.is_none() {
            self.inner = Some(jisuno_asr::transcribe::load(model)?);
        }
        Ok(())
    }

    fn decode_clip(&mut self, pcm_16k: &[f32]) -> Result<String> {
        let engine = self
            .inner
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("decode with no model loaded"))?;
        let mut text = String::new();
        engine.transcribe(pcm_16k, &mut |seg: &str| {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(seg);
        })?;
        Ok(text)
    }

    fn unload(&mut self) {
        self.inner = None;
    }
}

enum ToWorker {
    Load { gen: u64, name: String },
    Cancel { gen: u64 },
    Unload,
    Transcribe { id: u64, source: AudioSource },
}

enum AudioSource {
    Mic { rate: u32, pcm: Vec<f32> },
    Wav { path: PathBuf },
}

enum FromWorker {
    Warm {
        gen: u64,
    },
    LoadFailed {
        gen: u64,
        err: String,
    },
    Transcript {
        id: u64,
        text: String,
        decode_ms: u64,
    },
    NotReady {
        id: u64,
    },
    PendingFailed {
        id: u64,
        err: String,
    },
}

fn worker_main(rx: mpsc::Receiver<ToWorker>, tx: mpsc::Sender<FromWorker>) {
    let mut engine = ParakeetEngine { inner: None };
    // The load this generation installed, if any. Messages are processed in
    // order and loads block, so a Transcribe sent while the main thread
    // believes the model is loading simply queues behind the Load and
    // decodes as soon as it lands (only the remainder adds latency).
    let mut ready_gen: Option<u64> = None;
    let send = |msg: FromWorker| {
        let _ = tx.send(msg);
    };

    while let Ok(msg) = rx.recv() {
        match msg {
            ToWorker::Load { gen, name } => {
                let model = match AsrModel::parse(&name) {
                    Ok(model) => model,
                    Err(e) => {
                        send(FromWorker::LoadFailed {
                            gen,
                            err: format!("{e:#}"),
                        });
                        continue;
                    }
                };
                if !model.is_ready() {
                    send(FromWorker::LoadFailed {
                        gen,
                        err: format!(
                            "{} is not downloaded — run: jisuno setup --model {}",
                            model.id(),
                            model.id()
                        ),
                    });
                    continue;
                }
                match engine.ensure_loaded(model) {
                    Ok(()) => {
                        ready_gen = Some(gen);
                        send(FromWorker::Warm { gen });
                    }
                    Err(e) => {
                        ready_gen = None;
                        send(FromWorker::LoadFailed {
                            gen,
                            err: format!("{e:#}"),
                        });
                    }
                }
            }
            // A tap cancels the load its press started: by the time this is
            // processed the load has either failed (nothing to do) or
            // succeeded (unload it — the owner moved on). The main thread
            // bumps its generation so the stale Warm is ignored. The next
            // hold reloads; that waste is the documented cost of a tap.
            ToWorker::Cancel { gen } => {
                if ready_gen == Some(gen) {
                    engine.unload();
                    ready_gen = None;
                }
            }
            ToWorker::Unload => {
                engine.unload();
                ready_gen = None;
            }
            ToWorker::Transcribe { id, source } => {
                if engine.inner.is_some() {
                    decode_and_send(&mut engine, &send, id, source);
                } else {
                    send(FromWorker::NotReady { id });
                }
            }
        }
    }
}

fn decode_and_send(
    engine: &mut ParakeetEngine,
    send: &impl Fn(FromWorker),
    id: u64,
    source: AudioSource,
) {
    let started = Instant::now();
    let pcm: Vec<f32> = match source {
        AudioSource::Mic { rate, pcm } => audio::resample_to_16k(&pcm, rate),
        AudioSource::Wav { path } => match audio::load_wav_16k_mono(&path) {
            Ok(pcm) => pcm,
            Err(e) => {
                send(FromWorker::PendingFailed {
                    id,
                    err: format!("debug wav failed: {e:#}"),
                });
                return;
            }
        },
    };
    match engine.decode_clip(&pcm) {
        Ok(text) => send(FromWorker::Transcript {
            id,
            text,
            decode_ms: started.elapsed().as_millis() as u64,
        }),
        Err(e) => send(FromWorker::PendingFailed {
            id,
            err: format!("{e:#}"),
        }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelState {
    Unloaded,
    Loading { gen: u64 },
    Warm,
    Failed,
}

struct PendingTranscribe {
    id: u64,
    mode: Mode,
    key_up: Instant,
}

struct HudWindow {
    handle: gpui::AnyWindowHandle,
    dismiss_at: Option<Instant>,
    sticky: bool,
}

struct ListeningUi {
    dictate: bool,
    hands_free: bool,
    level: f32,
    loading_since: Option<Instant>,
    countdown_ms: Option<u64>,
}

struct Watched {
    path: PathBuf,
    mtime: Option<SystemTime>,
}

impl Watched {
    fn changed(&mut self) -> bool {
        let mtime = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok();
        if mtime != self.mtime {
            self.mtime = mtime;
            true
        } else {
            false
        }
    }
}

// Read by the popover (comes later); until then entries are logged on push.
#[allow(dead_code)]
struct RecentEntry {
    text: String,
    tag: Option<String>,
}

const TICK_MS: u64 = 20;
const CONFIG_POLL_MS: u64 = 500;
const SECURE_POLL_MS: u64 = 1000;

struct AppState {
    start: Instant,
    machine: Arc<Mutex<HoldMachine>>,
    bindings: Keybindings,
    settings: Settings,
    settings_watch: Watched,
    keybindings_watch: Watched,
    theme_watch: Watched,
    last_poll: Instant,
    last_secure_poll: Instant,
    pending: Option<PendingTranscribe>,
    capture: Option<Capture>,
    mic_open: bool,
    mic_rate: u32,
    buffer: Vec<f32>,
    heard_speech: bool,
    handsfree_vad: Option<jisuno_logic::vad::Vad>,
    level: f32,
    hud: Option<HudWindow>,
    hud_queue: VecDeque<UiHud>,
    listening: Option<ListeningUi>,
    show_latency: bool,
    handsfree_armed: bool,
    perm_granted: [bool; 3],
    model_present: bool,
    model_state: ModelState,
    model_gen: u64,
    model_name: String,
    last_active: Instant,
    target: Option<AppInfo>,
    to_worker: mpsc::Sender<ToWorker>,
    from_worker: mpsc::Receiver<FromWorker>,
    tap_rx: Option<mpsc::Receiver<KeyEvent>>,
    tap: Option<KeyTap>,
    menu_rx: mpsc::Receiver<MenuAction>,
    status: StatusItem,
    model_line: String,
    perm_line: [String; 3],
    recent: VecDeque<RecentEntry>,
    announced_secure: Vec<String>,
    announced_bluetooth: bool,
    bluetooth_now: bool,
    missing_device_note: Option<String>,
    secure_now: bool,
    last_config_error: Option<String>,
    debug_wav: Option<PathBuf>,
    debug_hold: Option<(Instant, Instant, bool)>,
    quit_requested: bool,
}

impl AppState {
    fn log(&self, msg: &str) {
        let e = self.start.elapsed();
        eprintln!(
            "[{:02}:{:02}.{:03}] {msg}",
            e.as_secs() / 60,
            e.as_secs() % 60,
            e.subsec_millis()
        );
    }

    fn input(&mut self, cx: &mut App, ev: Input, tag: &str) {
        let now = self.now_ms();
        let effects = self
            .machine
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .handle(ev, now);
        self.log(&format!("hold {tag} -> {} effect(s)", effects.len()));
        self.exec_effects(cx, effects);
    }

    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn machine_captures(&self, key: CapturedKey) -> bool {
        self.machine
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .captures(key)
    }
}

// --- Startup: config files, permissions -----------------------------------

fn embedded_app_files() -> Vec<EmbeddedFile> {
    let mut files: Vec<EmbeddedFile> = settings::embedded_settings_files().to_vec();
    files.push(EmbeddedFile {
        name: "keybindings.json",
        contents: include_str!("../../jisuno-logic/defaults/keybindings.json"),
    });
    files.push(EmbeddedFile {
        name: "schemas/keybindings.schema.json",
        contents: include_str!("../../jisuno-logic/schemas/keybindings.schema.json"),
    });
    files.push(EmbeddedFile {
        name: "theme.json",
        contents: include_str!("../../jisuno-logic/defaults/theme.json"),
    });
    files.push(EmbeddedFile {
        name: "schemas/theme.schema.json",
        contents: include_str!("../../jisuno-logic/schemas/theme.schema.json"),
    });
    files.push(EmbeddedFile {
        name: "voice-control-config.json",
        contents: include_str!("../../jisuno-logic/defaults/voice-control-config.json"),
    });
    files.push(EmbeddedFile {
        name: "schemas/voice-control-config.schema.json",
        contents: include_str!("../../jisuno-logic/schemas/voice-control-config.schema.json"),
    });
    files
}

fn read_file(path: &std::path::Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}

fn stamp(start: Instant, msg: &str) {
    let e = start.elapsed();
    eprintln!(
        "[{:02}:{:02}.{:03}] {msg}",
        e.as_secs() / 60,
        e.as_secs() % 60,
        e.subsec_millis()
    );
}

/// Run the menu bar app. Returns when the user quits.
pub fn run(opts: AppOptions) {
    let start = Instant::now();

    let dir = match ConfigDir::resolve() {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("jisuno app: {e:#}");
            std::process::exit(1);
        }
    };
    match dir.ensure_first_run(&embedded_app_files()) {
        Ok(written) => {
            for path in written {
                stamp(start, &format!("wrote {}", path.display()));
            }
        }
        Err(e) => {
            eprintln!("jisuno app: first run failed: {e:#}");
            std::process::exit(1);
        }
    }

    let mut settings_holder = Holder {
        current: Settings::default(),
        last_error: None,
    };
    let mut bindings_holder = Holder {
        current: keybindings::parse(include_str!("../../jisuno-logic/defaults/keybindings.json"))
            .unwrap_or_else(|e| {
                eprintln!("jisuno app: embedded keybindings invalid: {e:?}");
                std::process::exit(1);
            }),
        last_error: None,
    };
    let mut theme_holder = Holder {
        current: jisuno_ui::tokens::ThemeJson::default(),
        last_error: None,
    };

    let mut startup_errors: Vec<UiHud> = Vec::new();
    let reload_configs = |settings_holder: &mut Holder<Settings>,
                          bindings_holder: &mut Holder<Keybindings>,
                          theme_holder: &mut Holder<jisuno_ui::tokens::ThemeJson>,
                          startup_errors: &mut Vec<UiHud>| {
        match read_file(&dir.settings_path()).map(|t| Settings::parse(&t)) {
            Ok(Ok(settings)) => settings_holder.set(settings),
            Ok(Err(d)) => {
                let msg = format!("{d} · using last good config");
                stamp(start, &format!("config error: {msg}"));
                if settings_holder.err(msg.clone()) {
                    startup_errors.push(UiHud::ConfigError {
                        location: "settings.json".to_string(),
                        message: msg,
                    });
                }
            }
            Err(e) => stamp(start, &format!("settings unreadable: {e:#}")),
        }
        match read_file(&dir.keybindings_path()).map(|t| keybindings::parse_with_warnings(&t)) {
            Ok(Ok((bindings, warnings))) => {
                for w in &warnings {
                    stamp(
                        start,
                        &format!(
                            "keybindings warning: keybindings.json:{}: {}",
                            w.line, w.message
                        ),
                    );
                }
                bindings_holder.set(bindings);
            }
            Ok(Err(errors)) => {
                for d in errors {
                    let msg = format!(
                        "keybindings.json:{}: {} · using last good config",
                        d.line, d.message
                    );
                    stamp(start, &format!("config error: {msg}"));
                    if bindings_holder.err(msg.clone()) {
                        startup_errors.push(UiHud::ConfigError {
                            location: "keybindings.json".to_string(),
                            message: msg,
                        });
                    }
                }
            }
            Err(e) => stamp(start, &format!("keybindings unreadable: {e:#}")),
        }
        match read_file(&dir.theme_path()).map(|t| {
            serde_json::from_str::<serde_json::Value>(&t)
                .map_err(|e| format!("invalid JSON: {e}"))
                .and_then(|v| {
                    jisuno_ui::tokens::ThemeJson::from_value(&v).map_err(|e| format!("{e:#}"))
                })
        }) {
            Ok(Ok(theme)) => theme_holder.set(theme),
            Ok(Err(message)) => {
                let full = format!("theme.json: {message} · using last good config");
                stamp(start, &format!("config error: {full}"));
                if theme_holder.err(full.clone()) {
                    startup_errors.push(UiHud::ConfigError {
                        location: "theme.json".to_string(),
                        message: full,
                    });
                }
            }
            Err(e) => stamp(start, &format!("theme unreadable: {e:#}")),
        }
    };
    reload_configs(
        &mut settings_holder,
        &mut bindings_holder,
        &mut theme_holder,
        &mut startup_errors,
    );

    // Permissions: request what was never asked, then snapshot the rest.
    let mut missing: Vec<(Pane, UiHud)> = Vec::new();
    let mic = permissions::microphone();
    if mic == PermissionState::NotDetermined {
        stamp(start, "microphone: not determined — requesting");
        match permissions::request_microphone() {
            Ok(state) => stamp(start, &format!("microphone prompt answered: {state:?}")),
            Err(e) => stamp(start, &format!("microphone prompt failed: {e:#}")),
        }
    }
    if permissions::microphone() != PermissionState::Granted {
        stamp(start, "microphone: not granted");
        missing.push((Pane::Microphone, UiHud::MicrophoneOff));
    }
    if permissions::accessibility() != PermissionState::Granted {
        stamp(start, "accessibility: not granted — requesting");
        match permissions::request_accessibility() {
            Ok(true) => stamp(start, "accessibility: granted"),
            _ => {
                stamp(start, "accessibility: not granted");
                missing.push((Pane::Accessibility, UiHud::InsertBlocked));
            }
        }
    }
    match permissions::input_monitoring() {
        PermissionState::NotDetermined | PermissionState::Denied => {
            stamp(start, "input monitoring: prompting");
            let _ = permissions::request_input_monitoring();
            if permissions::input_monitoring() != PermissionState::Granted {
                stamp(start, "input monitoring: not granted");
                missing.push((
                    Pane::InputMonitoring,
                    UiHud::ConfigError {
                        location: "permissions".to_string(),
                        message: "Input Monitoring is off · open System Settings".to_string(),
                    },
                ));
            }
        }
        PermissionState::Granted => {}
    }
    for (_, hud) in &missing {
        startup_errors.push(hud.clone());
    }

    let machine = Arc::new(Mutex::new(HoldMachine::new(Timing {
        dictation_limit_s: u64::from(settings_holder.current.dictation_limit_s),
        ..Timing::default()
    })));
    let (menu_tx, menu_rx) = mpsc::channel();
    let (to_worker, worker_rx) = mpsc::channel();
    let (worker_tx, from_worker) = mpsc::channel();
    std::thread::spawn(move || worker_main(worker_rx, worker_tx));

    let debug_hold = opts.debug_hold_ms.map(|ms| {
        let down = Instant::now() + Duration::from_millis(1000);
        (down, down + Duration::from_millis(ms), false)
    });

    gpui_platform::application().run(move |cx: &mut App| {
        cx.set_activation_policy(ActivationPolicy::Accessory);
        if let Err(e) = jisuno_ui::hud_view::load_system_fonts(cx) {
            stamp(start, &format!("system fonts: {e:#}"));
        }
        let status = match StatusItem::new(menu_tx) {
            Ok(status) => status,
            Err(e) => {
                eprintln!("jisuno app: status item failed: {e:#}");
                std::process::exit(1);
            }
        };
        for (pane, _) in &missing {
            status.set_permission(*pane, false);
        }
        status.set_permission(
            Pane::Microphone,
            !missing.iter().any(|(p, _)| *p == Pane::Microphone),
        );
        status.set_permission(
            Pane::Accessibility,
            !missing.iter().any(|(p, _)| *p == Pane::Accessibility),
        );
        status.set_permission(
            Pane::InputMonitoring,
            !missing.iter().any(|(p, _)| *p == Pane::InputMonitoring),
        );

        let tap_config = tap_config_for(&bindings_holder.current);
        let machine_w = Arc::clone(&machine);
        let should_capture: ShouldCapture = Arc::new(move |captured: Captured| {
            let machine = machine_w.lock().unwrap_or_else(|e| e.into_inner());
            match captured {
                Captured::Escape => machine.captures(CapturedKey::Escape),
                Captured::Return => machine.captures(CapturedKey::Return),
                Captured::Chord(_) => true,
            }
        });
        let (tap, tap_rx) = match KeyTap::start(tap_config, should_capture) {
            Ok((tap, rx)) => (Some(tap), Some(rx)),
            Err(e) => {
                stamp(start, &format!("event tap failed: {e:#}"));
                if opts.debug_wav.is_none() && opts.debug_hold_ms.is_none() {
                    eprintln!("jisuno app: grant Input Monitoring, then relaunch");
                    std::process::exit(1);
                }
                stamp(start, "continuing tapless for the debug run");
                (None, None)
            }
        };

        let settings = settings_holder.current.clone();
        let state = Rc::new(RefCell::new(AppState {
            start,
            machine,
            bindings: bindings_holder.current.clone(),
            settings: settings.clone(),
            settings_watch: Watched {
                path: dir.settings_path(),
                mtime: std::fs::metadata(dir.settings_path())
                    .and_then(|m| m.modified())
                    .ok(),
            },
            keybindings_watch: Watched {
                path: dir.keybindings_path(),
                mtime: std::fs::metadata(dir.keybindings_path())
                    .and_then(|m| m.modified())
                    .ok(),
            },
            theme_watch: Watched {
                path: dir.theme_path(),
                mtime: std::fs::metadata(dir.theme_path())
                    .and_then(|m| m.modified())
                    .ok(),
            },
            last_poll: Instant::now(),
            last_secure_poll: Instant::now(),
            pending: None,
            capture: None,
            mic_open: false,
            mic_rate: 16_000,
            buffer: Vec::new(),
            heard_speech: false,
            handsfree_vad: None,
            level: 0.0,
            hud: None,
            hud_queue: startup_errors.into_iter().collect(),
            listening: None,
            show_latency: theme_holder.current.show_latency,
            handsfree_armed: false,
            perm_granted: [true, true, true],
            model_present: AsrModel::parse(&model_id(&settings))
                .map(|m| m.is_ready())
                .unwrap_or(false),
            model_state: ModelState::Unloaded,
            model_gen: 0,
            model_name: model_id(&settings),
            last_active: Instant::now(),
            target: None,
            to_worker,
            from_worker,
            tap_rx,
            tap,
            menu_rx,
            status,
            model_line: String::new(),
            perm_line: [String::new(), String::new(), String::new()],
            recent: VecDeque::with_capacity(5),
            announced_secure: Vec::new(),
            announced_bluetooth: false,
            bluetooth_now: false,
            missing_device_note: None,
            secure_now: false,
            last_config_error: None,
            debug_wav: opts.debug_wav,
            debug_hold,
            quit_requested: false,
        }));
        if settings.model_load == settings::ModelLoad::AtLogin {
            state.borrow_mut().start_load(cx, "at-login");
        }
        let tick_state = Rc::clone(&state);
        cx.spawn(async move |cx: &mut AsyncApp| {
            // The first tick shows any startup errors immediately.
            cx.update(|cx| tick_state.borrow_mut().tick(cx));
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(TICK_MS))
                    .await;
                let quit = cx.update(|cx| tick_state.borrow_mut().tick(cx));
                if quit {
                    break;
                }
            }
        })
        .detach();
    });
}

fn model_id(settings: &Settings) -> String {
    match settings.model {
        settings::Model::ParakeetV3Int8 => "parakeet-tdt-0.6b-v3-int8".to_string(),
        settings::Model::ParakeetV2Int8 => "parakeet-tdt-0.6b-v2-int8".to_string(),
    }
}

fn tap_config_for(bindings: &Keybindings) -> TapConfig {
    let mut holds = Vec::new();
    let mut chords = Vec::new();
    for binding in &bindings.bindings {
        if binding.hold {
            if let Key::Hold(side) = binding.key {
                holds.push(RightKey::from(side));
            }
        } else if binding.when.is_none() {
            if let Key::Chord { .. } = binding.key {
                chords.push(binding.key.clone());
            }
        }
    }
    TapConfig { holds, chords }
}

fn hold_kind_for(bindings: &Keybindings, key: RightKey) -> Option<HoldKind> {
    let wanted = Key::Hold(key.into());
    bindings
        .bindings
        .iter()
        .find(|b| b.hold && b.key == wanted)
        .and_then(|b| match b.command {
            Command::Dictate => Some(HoldKind::Dictate),
            Command::VoiceCommand => Some(HoldKind::Command),
            _ => None,
        })
}

// --- The 20 ms driver -------------------------------------------------------

impl AppState {
    fn tick(&mut self, cx: &mut App) -> bool {
        if self.quit_requested {
            self.shutdown();
            cx.quit();
            return true;
        }
        self.drain_menu(cx);
        self.drain_tap(cx);
        self.drain_worker(cx);
        let effects = self
            .machine
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .tick(self.now_ms());
        if !effects.is_empty() {
            self.log(&format!("hold tick -> {} effect(s)", effects.len()));
        }
        self.exec_effects(cx, effects);
        self.drain_mic();
        self.poll_config(cx);
        self.poll_secure(cx);
        self.poll_idle_unload();
        self.refresh_hud_level(cx);
        self.refresh_chrome();
        if let Some((down, up, sent)) = self.debug_hold {
            let now = Instant::now();
            if !sent && now >= down {
                self.debug_hold = Some((down, up, true));
                self.log("debug hold down (simulated)");
                self.begin_hold(cx, HoldKind::Dictate);
            } else if sent && now >= up {
                self.debug_hold = None;
                self.log("debug hold up (simulated)");
                self.input(cx, Input::HoldUp(HoldKind::Dictate), "up dictate (debug)");
            }
        }
        false
    }

    fn shutdown(&mut self) {
        self.log("quitting");
        let _ = self.to_worker.send(ToWorker::Unload);
        if let Some(tap) = self.tap.take() {
            if let Err(e) = tap.shutdown() {
                self.log(&format!("tap shutdown: {e:#}"));
            }
        }
        if let Some(hud) = self.hud.take() {
            // Best effort: the process is going away anyway.
            let _ = hud;
        }
    }

    // --- Menu ---

    fn drain_menu(&mut self, cx: &mut App) {
        while let Ok(action) = self.menu_rx.try_recv() {
            match action {
                MenuAction::WarmModel => self.start_load(cx, "menu"),
                MenuAction::UnloadModel => self.unload_model("menu"),
                MenuAction::OpenPane(pane) => {
                    if let Err(e) = permissions::open_settings_pane(pane) {
                        self.log(&format!("open settings pane: {e:#}"));
                    }
                }
                MenuAction::Quit => self.quit_requested = true,
            }
        }
    }

    // --- Event tap ---

    fn drain_tap(&mut self, cx: &mut App) {
        let mut events = Vec::new();
        if let Some(rx) = &self.tap_rx {
            while let Ok(ev) = rx.try_recv() {
                events.push(ev);
            }
        }
        for ev in events {
            self.log(&format!("tap {ev:?}"));
            match ev {
                KeyEvent::HoldDown(key) => {
                    self.target = frontmost::frontmost_app();
                    if let Some(app) = &self.target {
                        self.log(&format!("frontmost on hold: {app:?}"));
                    }
                    match hold_kind_for(&self.bindings, key) {
                        Some(kind) => self.begin_hold(cx, kind),
                        None => self.log("tap: hold key with no dictate/command binding — ignored"),
                    }
                }
                KeyEvent::HoldUp(key) => {
                    if let Some(kind) = hold_kind_for(&self.bindings, key) {
                        self.input(cx, Input::HoldUp(kind), "up");
                    }
                }
                KeyEvent::OtherKeyDown => self.input(cx, Input::OtherKeyDown, "other-key"),
                KeyEvent::Chord(key) => self.on_chord(cx, &key),
                KeyEvent::Escape => {
                    if self.machine_captures(CapturedKey::Escape) {
                        self.input(cx, Input::Cancel, "cancel (esc)");
                    }
                }
                KeyEvent::Return => {
                    if self.machine_captures(CapturedKey::Return) {
                        self.input(cx, Input::Confirm, "confirm (return)");
                    }
                }
                KeyEvent::TapReenabled => {
                    self.log("tap re-enabled: keys may have been missed");
                    self.input(cx, Input::Resync, "resync");
                }
            }
        }
    }

    fn on_chord(&mut self, cx: &mut App, key: &Key) {
        match self.bindings.lookup(key, None) {
            Some(Command::HandsFreeToggle) => {
                self.target = frontmost::frontmost_app();
                if self.check_hold_ready(cx) {
                    self.handsfree_armed = true;
                    self.input(cx, Input::HandsFreeToggle, "hands-free toggle");
                }
            }
            Some(Command::Warm) => self.start_load(cx, "warm key"),
            Some(Command::Unload) => self.unload_model("unload key"),
            Some(Command::Settings) => self.log("settings window comes later"),
            Some(Command::Menu) => self.log("menu popover comes later"),
            Some(command) => self.log(&format!("chord for {command:?} — no app action yet")),
            None => {}
        }
    }

    /// HoldDown path with the spec's pre-checks: the machine never sees a
    /// hold the host cannot serve (missing mic/model), so a dead press shows
    /// the error HUD and is silently ignored on release.
    fn begin_hold(&mut self, cx: &mut App, kind: HoldKind) {
        if !self.check_hold_ready(cx) {
            return;
        }
        let tag = match kind {
            HoldKind::Dictate => "down dictate",
            HoldKind::Command => "down command",
        };
        self.input(cx, Input::HoldDown(kind), tag);
    }

    fn check_hold_ready(&mut self, cx: &mut App) -> bool {
        if secure_input::is_secure_input_enabled() {
            let holder = secure_input::secure_input_holder();
            let name = holder
                .as_ref()
                .and_then(|h| h.name.clone())
                .unwrap_or_else(|| "unknown".to_string());
            if !self.announced_secure.contains(&name) {
                self.announced_secure.push(name.clone());
                self.show_ephemeral(
                    cx,
                    UiHud::SecureInput { app: name.clone() },
                    &format!("secure input on ({name})"),
                );
            } else {
                self.log(&format!(
                    "secure input on ({name}) · keys paused (already shown)"
                ));
            }
            return false;
        }
        if self.debug_wav.is_none() && default_input_missing() {
            self.show_ephemeral(cx, UiHud::NoMicrophone, "no microphone");
            return false;
        }
        if !self.model_files_present() {
            self.show_ephemeral(cx, UiHud::ModelMissing, "model missing");
            return false;
        }
        true
    }

    fn model_files_present(&self) -> bool {
        AsrModel::parse(&self.model_name)
            .map(|m| m.is_ready())
            .unwrap_or(false)
    }

    // --- Mic ---

    fn open_mic(&mut self, _cx: &mut App) {
        self.mic_open = true;
        self.buffer.clear();
        self.heard_speech = false;
        self.level = 0.0;
        self.resolve_audio_notes();
        if self.debug_wav.is_some() {
            self.log("mic: debug wav substitutes for the microphone");
            return;
        }
        match jisuno_audio::mic::start() {
            Ok(capture) => {
                self.mic_rate = capture.input_rate;
                self.capture = Some(capture);
                self.log(&format!("mic open ({} Hz)", self.mic_rate));
            }
            Err(e) => {
                self.log(&format!("mic open failed: {e:#}"));
                self.mic_open = false;
                self.input_no_cx(Input::Resync, "resync (mic failed)");
            }
        }
        // Speech detection for hold sessions: skip the decode round-trip when
        // the press captured no speech at all.
        self.handsfree_vad = None;
        if self.expect_handsfree() {
            self.handsfree_vad = Some(jisuno_logic::vad::Vad::new(u64::from(
                self.settings.hands_free_silence_ms,
            )));
        }
    }

    /// True when the press that just opened the mic was the hands-free
    /// toggle. The machine does not report the mode in OpenMic, so the flag
    /// is set when the toggle is fed and consumed here.
    fn expect_handsfree(&mut self) -> bool {
        // Armed by HandsFreeToggle handling and consumed when the mic opens
        // for that press. A stray arm (toggle ignored mid-hold) is harmless:
        // the silence input is ignored outside hands-free state.
        std::mem::replace(&mut self.handsfree_armed, false)
    }

    fn close_mic(&mut self, discard: bool) {
        self.drain_mic();
        if let Some(capture) = self.capture.take() {
            capture.stop();
        }
        self.mic_open = false;
        self.handsfree_vad = None;
        if discard {
            self.buffer.clear();
            self.heard_speech = false;
            self.log("mic closed (discarded)");
        } else {
            self.log(&format!("mic closed ({} samples kept)", self.buffer.len()));
        }
    }

    fn drain_mic(&mut self) {
        let mut chunks = Vec::new();
        if let Some(capture) = &self.capture {
            while let Ok(chunk) = capture.chunks.try_recv() {
                chunks.push(chunk);
            }
        }
        for chunk in chunks {
            let rms = jisuno_logic::vad::rms(&chunk);
            if rms > self.level {
                self.level = rms;
            }
            if !chunk.iter().all(|s| *s == 0.0) && rms > 0.02 {
                self.heard_speech = true;
            }
            if let Some(vad) = &mut self.handsfree_vad {
                if vad.push(&chunk) == jisuno_logic::vad::VadEvent::SilenceTimeout {
                    self.handsfree_vad = None;
                    self.input_no_cx(Input::SilenceDetected, "hands-free silence");
                }
            }
            self.buffer.extend_from_slice(&chunk);
        }
    }

    /// Resolve the Bluetooth / missing-device notes for this utterance (H16/H17).
    fn resolve_audio_notes(&mut self) {
        self.bluetooth_now = false;
        self.missing_device_note = None;
        let wanted = self.settings.input_device.clone();
        if wanted != "system" {
            match find_input_device(&wanted) {
                Some(name) => {
                    if is_bluetooth_name(&name) {
                        self.bluetooth_now = true;
                    }
                    // Named-device capture is not wired yet (mic::start opens
                    // the default); fall back with the H17 note either way.
                    self.missing_device_note = Some(wanted);
                }
                None => self.missing_device_note = Some(wanted),
            }
            return;
        }
        if let Some(name) = default_input_name() {
            if is_bluetooth_name(&name) {
                self.bluetooth_now = true;
            }
        }
    }

    // --- Effects ---

    fn exec_effects(&mut self, cx: &mut App, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::OpenMic => self.open_mic(cx),
                Effect::CloseMic { discard } => self.close_mic(discard),
                Effect::StartModelLoad => self.start_load(cx, "hold"),
                Effect::CancelModelLoad => self.cancel_load(),
                Effect::ShowHud(state) => self.show_machine_hud(cx, state),
                Effect::HideHud => self.hide_hud(cx),
                Effect::Transcribe { mode, id } => self.begin_transcribe(cx, mode, id),
                Effect::RunCommand => self.log("run command: command mode comes later"),
                Effect::ShowConfirm { label } => {
                    self.log(&format!("confirm prompt (stub): {label}"))
                }
                Effect::PassThrough => {}
                Effect::Countdown { secs_left } => {
                    if let Some(listening) = &mut self.listening {
                        listening.countdown_ms = Some(secs_left * 1000);
                    }
                    self.log(&format!("countdown {secs_left}s left"));
                }
            }
        }
    }

    /// Machine input from a context without `&mut App` (mic drain,
    /// secure-input poll): execute App-free effects inline. HideHud cannot
    /// close the window without `App`, so it marks the HUD for closing on
    /// the next tick instead.
    fn input_no_cx(&mut self, ev: Input, tag: &str) {
        let effects = self
            .machine
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .handle(ev, self.now_ms());
        self.log(&format!("hold {tag} -> {} effect(s)", effects.len()));
        for effect in effects {
            match effect {
                Effect::CloseMic { discard } => self.close_mic(discard),
                Effect::CancelModelLoad => self.cancel_load(),
                Effect::OpenMic => self.open_mic_no_cx(),
                Effect::HideHud => {
                    self.listening = None;
                    if let Some(hud) = &mut self.hud {
                        hud.sticky = false;
                        hud.dismiss_at = Some(Instant::now());
                    }
                }
                _ => self.log(&format!("hold {tag}: deferred effect {effect:?} to tick")),
            }
        }
    }

    fn open_mic_no_cx(&mut self) {
        // Only reached from Resync paths; keep the state consistent without
        // touching UI: mic stays closed until the next real press.
        self.mic_open = false;
        self.buffer.clear();
    }

    fn show_machine_hud(&mut self, cx: &mut App, state: jisuno_logic::hold::HudState) {
        use jisuno_logic::hold::HudState as LogicHud;
        match state {
            LogicHud::Listening(mode) => {
                let dictate = mode == Mode::Dictate;
                self.listening = Some(ListeningUi {
                    dictate,
                    hands_free: false,
                    level: 0.5,
                    loading_since: (self.model_state != ModelState::Warm).then(Instant::now),
                    countdown_ms: None,
                });
                if dictate {
                    self.log("hud: listening dictate");
                } else {
                    self.log("hud: listening command");
                }
                self.open_listening_hud(cx);
            }
            LogicHud::HandsFree => {
                self.listening = Some(ListeningUi {
                    dictate: true,
                    hands_free: true,
                    level: 0.5,
                    loading_since: (self.model_state != ModelState::Warm).then(Instant::now),
                    countdown_ms: None,
                });
                self.log("hud: hands-free");
                self.open_listening_hud(cx);
            }
        }
    }

    // --- Transcribe ---

    fn begin_transcribe(&mut self, cx: &mut App, mode: Mode, id: u64) {
        let key_up = Instant::now();
        self.pending = Some(PendingTranscribe { id, mode, key_up });
        self.last_active = Instant::now();
        match mode {
            Mode::Dictate => {
                if !self.heard_speech && self.debug_wav.is_none() {
                    self.log("no speech captured — skipping decode");
                    self.pending = None;
                    self.input(cx, Input::Finished { hud: None, id }, "finished (silence)");
                    return;
                }
                if self.model_state == ModelState::Failed {
                    self.log("transcribe with failed load — model missing HUD");
                    self.pending = None;
                    self.input(
                        cx,
                        Input::Finished { hud: None, id },
                        "finished (load failed)",
                    );
                    self.show_ephemeral(cx, UiHud::ModelMissing, "model load failed");
                    return;
                }
                let source = match &self.debug_wav {
                    Some(path) => AudioSource::Wav { path: path.clone() },
                    None => AudioSource::Mic {
                        rate: self.mic_rate,
                        pcm: std::mem::take(&mut self.buffer),
                    },
                };
                self.show_decoding(cx, true, self.model_state != ModelState::Warm);
                if self
                    .to_worker
                    .send(ToWorker::Transcribe { id, source })
                    .is_err()
                {
                    self.log("model worker gone");
                    self.pending = None;
                    self.input(
                        cx,
                        Input::Finished { hud: None, id },
                        "finished (worker gone)",
                    );
                }
            }
            Mode::Command => {
                let source = match &self.debug_wav {
                    Some(path) => AudioSource::Wav { path: path.clone() },
                    None => AudioSource::Mic {
                        rate: self.mic_rate,
                        pcm: std::mem::take(&mut self.buffer),
                    },
                };
                self.show_decoding(cx, false, self.model_state != ModelState::Warm);
                if self
                    .to_worker
                    .send(ToWorker::Transcribe { id, source })
                    .is_err()
                {
                    self.pending = None;
                    self.input(
                        cx,
                        Input::Finished { hud: None, id },
                        "finished (worker gone)",
                    );
                }
            }
        }
    }

    fn on_transcript(&mut self, cx: &mut App, id: u64, text: String, decode_ms: u64) {
        let Some(pending) = self.pending.take() else {
            self.log(&format!("stale transcript {id} — ignored"));
            return;
        };
        if pending.id != id {
            self.log(&format!(
                "stale transcript {id} (want {}) — ignored",
                pending.id
            ));
            self.pending = Some(pending);
            return;
        }
        let key_up_ms = pending.key_up.elapsed().as_millis();
        self.last_active = Instant::now();
        match pending.mode {
            Mode::Dictate => {
                if text.trim().is_empty() {
                    self.input(cx, Input::Finished { hud: None, id }, "finished (empty)");
                    return;
                }
                match self.insert_text(&text) {
                    Ok(()) => {
                        let words = text.split_whitespace().count();
                        self.push_recent(text.clone(), None);
                        self.log(&format!(
                            "key-up -> text: {key_up_ms} ms (decode {decode_ms} ms)"
                        ));
                        self.input(cx, Input::TranscriptReady { text, id }, "transcript ready");
                        let latency = self.show_latency.then_some(key_up_ms as u64);
                        self.show_ephemeral(
                            cx,
                            UiHud::Inserted {
                                words: words as u32,
                                latency_ms: latency,
                            },
                            &format!("inserted {words} words"),
                        );
                    }
                    Err(kind) => {
                        self.input(
                            cx,
                            Input::TranscriptReady {
                                text: String::new(),
                                id,
                            },
                            "transcript ready (insert failed)",
                        );
                        let hud = match kind {
                            InsertFailure::FocusLost => UiHud::FocusLost,
                            InsertFailure::Blocked => UiHud::InsertBlocked,
                        };
                        self.show_ephemeral(cx, hud, "insert failed");
                    }
                }
            }
            Mode::Command => {
                self.input(
                    cx,
                    Input::Finished { hud: None, id },
                    "finished (command stub)",
                );
                self.log(&format!("command phrase (stub): {text:?}"));
                self.show_ephemeral(
                    cx,
                    UiHud::RanCommand {
                        label: "Commands coming soon".to_string(),
                    },
                    "command mode comes later",
                );
            }
        }
    }

    fn insert_text(&mut self, text: &str) -> Result<(), InsertFailure> {
        let pid = self.target.as_ref().map(|t| t.pid);
        let bundle = self.target.as_ref().and_then(|t| t.bundle_id.clone());
        let mut mode = match self.settings.insert {
            settings::Insert::Paste => InsertMode::Paste,
            settings::Insert::Type => InsertMode::Type,
        };
        if let Some(bundle) = bundle {
            if self.settings.insert_type_apps.contains(&bundle) {
                mode = InsertMode::Type;
            }
        }
        match insert::insert_text(text, pid, mode, self.settings.restore_clipboard) {
            Ok(()) => {
                // Paste leaves the restore to a background thread; outlive it.
                std::thread::sleep(Duration::from_millis(200));
                Ok(())
            }
            Err(e) => {
                if e.downcast_ref::<FocusLost>().is_some() {
                    self.log("focus lost: text in Recent");
                    self.push_recent(text.to_string(), Some("not inserted".to_string()));
                    Err(InsertFailure::FocusLost)
                } else {
                    self.log(&format!("insert failed: {e:#}"));
                    self.push_recent(text.to_string(), Some("not inserted".to_string()));
                    Err(InsertFailure::Blocked)
                }
            }
        }
    }

    // --- Model lifecycle ---

    fn start_load(&mut self, cx: &mut App, why: &str) {
        if self.model_state == ModelState::Warm {
            self.log(&format!("warm ({why}): already warm"));
            return;
        }
        if matches!(self.model_state, ModelState::Loading { gen } if gen == self.model_gen) {
            self.log(&format!("warm ({why}): already loading"));
            return;
        }
        if !self.model_files_present() {
            self.log(&format!("warm ({why}): model not downloaded"));
            self.show_ephemeral(cx, UiHud::ModelMissing, "model missing");
            return;
        }
        self.model_gen += 1;
        let gen = self.model_gen;
        self.model_state = ModelState::Loading { gen };
        self.machine
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .handle(Input::LoadStarted, self.now_ms());
        let _ = self.to_worker.send(ToWorker::Load {
            gen,
            name: self.model_name.clone(),
        });
        self.log(&format!("model load started ({why})"));
    }

    fn cancel_load(&mut self) {
        if let ModelState::Loading { gen } = self.model_state {
            // Settle locally first so the in-flight Warm is ignored; the
            // worker unloads whatever the press started.
            self.model_gen += 1;
            self.model_state = ModelState::Unloaded;
            let _ = self.to_worker.send(ToWorker::Cancel { gen });
            self.log("model load cancelled (tap)");
        }
    }

    fn unload_model(&mut self, why: &str) {
        if self.model_state == ModelState::Warm {
            let _ = self.to_worker.send(ToWorker::Unload);
            self.model_state = ModelState::Unloaded;
            self.machine
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .set_model_warm(false);
            self.log(&format!("model unloaded ({why})"));
        } else {
            self.log(&format!("unload ({why}): nothing loaded"));
        }
    }

    fn poll_idle_unload(&mut self) {
        let mins = self.settings.unload_after_idle_min;
        if mins == 0
            || self.model_state != ModelState::Warm
            || self.pending.is_some()
            || self.mic_open
        {
            return;
        }
        if self.last_active.elapsed() >= Duration::from_secs(u64::from(mins) * 60) {
            self.unload_model("idle");
        }
    }

    fn drain_worker(&mut self, cx: &mut App) {
        while let Ok(msg) = self.from_worker.try_recv() {
            match msg {
                FromWorker::Warm { gen } => {
                    if gen != self.model_gen {
                        continue;
                    }
                    self.model_state = ModelState::Warm;
                    self.last_active = Instant::now();
                    self.machine
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .handle(Input::ModelWarm, self.now_ms());
                    self.log("model warm");
                    if !self.mic_open && self.pending.is_none() {
                        self.show_ephemeral(cx, UiHud::ModelReady, "model ready");
                    }
                }
                FromWorker::LoadFailed { gen, err } => {
                    if gen != self.model_gen {
                        continue;
                    }
                    self.model_state = ModelState::Failed;
                    self.machine
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .handle(Input::LoadFailed, self.now_ms());
                    self.log(&format!("model load failed: {err}"));
                }
                FromWorker::Transcript {
                    id,
                    text,
                    decode_ms,
                } => {
                    self.on_transcript(cx, id, text, decode_ms);
                }
                FromWorker::NotReady { id } => {
                    self.log(&format!("worker not ready for {id}"));
                    if self.pending.take().is_some() {
                        self.input(
                            cx,
                            Input::Finished { hud: None, id },
                            "finished (not ready)",
                        );
                        self.show_ephemeral(cx, UiHud::ModelMissing, "model not ready");
                    }
                }
                FromWorker::PendingFailed { id, err } => {
                    self.log(&format!("transcribe {id} failed: {err}"));
                    if self.pending.take().is_some() {
                        self.input(
                            cx,
                            Input::Finished { hud: None, id },
                            "finished (decode failed)",
                        );
                        self.show_ephemeral(cx, UiHud::ModelMissing, "decode failed");
                    }
                }
            }
        }
    }

    // --- HUD ---

    fn hud_size() -> HudSize {
        let (w, h) = HUD_WINDOW_SIZE;
        HudSize {
            width: f64::from(w),
            height: f64::from(h),
        }
    }

    fn display_metrics(cx: &App) -> Screen {
        let (mut width, mut height, mut top) = (1512.0, 982.0, 24.0);
        if let Some(display) = cx.primary_display() {
            width = display.bounds().size.width.to_f64();
            height = display.bounds().size.height.to_f64();
            top = display.visible_bounds().origin.y.to_f64();
        }
        Screen {
            width,
            height,
            usable_top: top,
        }
    }

    fn hud_enabled(&self) -> bool {
        self.settings.hud != settings::Hud::Off
    }

    fn listening_state(&self) -> Option<UiHud> {
        let l = self.listening.as_ref()?;
        let loading = l.loading_since.map(|since| {
            let frac = since.elapsed().as_secs_f32() / 1.5;
            frac.clamp(0.0, 0.95)
        });
        let partial = None;
        let countdown_ms = l.countdown_ms;
        let bluetooth = self.bluetooth_now && !self.announced_bluetooth;
        let missing_device = self.missing_device_note.clone();
        if l.dictate {
            Some(UiHud::ListeningDictate {
                level: l.level,
                hands_free: l.hands_free,
                loading,
                partial,
                countdown_ms,
                bluetooth,
                missing_device,
            })
        } else {
            Some(UiHud::ListeningCommand {
                level: l.level,
                loading,
                partial,
            })
        }
    }

    fn open_listening_hud(&mut self, cx: &mut App) {
        if !self.hud_enabled() {
            self.log("hud off — listening without a window");
            return;
        }
        if self.bluetooth_now && !self.announced_bluetooth {
            self.announced_bluetooth = true;
            self.log("bluetooth mic note (once per session)");
        }
        let Some(origin) = self.hud_origin(cx) else {
            return;
        };
        let Some(state) = self.listening_state() else {
            return;
        };
        self.replace_hud(cx, state, origin, true);
    }

    fn hud_origin(&self, cx: &App) -> Option<(f64, f64)> {
        let screen = Self::display_metrics(cx);
        let mouse = if self.settings.hud == settings::Hud::Cursor {
            mouse_position_gpui(screen.height)
        } else {
            None
        };
        hud_origin(self.settings.hud, screen, Self::hud_size(), mouse)
    }

    fn replace_hud(&mut self, cx: &mut App, state: UiHud, origin: (f64, f64), sticky: bool) {
        if let Some(hud) = self.hud.take() {
            close_hud_window(hud.handle, cx);
        }
        match open_hud_window(cx, state.clone(), origin) {
            Ok(handle) => {
                self.hud = Some(HudWindow {
                    handle: handle.into(),
                    dismiss_at: None,
                    sticky,
                });
                self.log(&format!(
                    "hud open ({}, {})",
                    origin.0 as i64, origin.1 as i64
                ));
            }
            Err(e) => self.log(&format!("hud open failed: {e:#}")),
        }
    }

    fn show_decoding(&mut self, cx: &mut App, dictate: bool, finishing_load: bool) {
        self.listening = None;
        if !self.hud_enabled() {
            return;
        }
        let Some(origin) = self.hud_origin(cx) else {
            return;
        };
        let state = UiHud::Decoding {
            mode: if dictate {
                UiMode::Dictate
            } else {
                UiMode::Command
            },
            finishing_load,
        };
        self.replace_hud(cx, state, origin, true);
    }

    fn show_ephemeral(&mut self, cx: &mut App, state: UiHud, tag: &str) {
        self.log(&format!("hud: {tag}"));
        if !self.hud_enabled() {
            return;
        }
        // A hold in progress is never replaced by an error line (§3.2).
        if self.listening.is_some() {
            self.hud_queue.push_back(state);
            return;
        }
        if let Some(hud) = &self.hud {
            if hud.sticky {
                self.hud_queue.push_back(state);
                return;
            }
        }
        let Some(origin) = self.hud_origin(cx) else {
            return;
        };
        let dismiss = jisuno_ui::hud_model::render_model(&state).dismiss_after_ms;
        if let Some(hud) = self.hud.take() {
            close_hud_window(hud.handle, cx);
        }
        match open_hud_window(cx, state, origin) {
            Ok(handle) => {
                self.hud = Some(HudWindow {
                    handle: handle.into(),
                    dismiss_at: dismiss.map(|ms| Instant::now() + Duration::from_millis(ms)),
                    sticky: false,
                });
            }
            Err(e) => self.log(&format!("hud open failed: {e:#}")),
        }
    }

    fn hide_hud(&mut self, cx: &mut App) {
        self.listening = None;
        if let Some(hud) = self.hud.take() {
            close_hud_window(hud.handle, cx);
            self.log("hud hidden");
        }
        self.drain_hud_queue(cx);
    }

    fn drain_hud_queue(&mut self, cx: &mut App) {
        if self.hud.is_some() || !self.hud_enabled() {
            return;
        }
        if let Some(state) = self.hud_queue.pop_front() {
            let dismiss = jisuno_ui::hud_model::render_model(&state).dismiss_after_ms;
            if let Some(origin) = self.hud_origin(cx) {
                match open_hud_window(cx, state, origin) {
                    Ok(handle) => {
                        self.hud = Some(HudWindow {
                            handle: handle.into(),
                            dismiss_at: dismiss
                                .map(|ms| Instant::now() + Duration::from_millis(ms)),
                            sticky: false,
                        });
                    }
                    Err(e) => self.log(&format!("hud open failed: {e:#}")),
                }
            }
        }
    }

    fn refresh_hud_level(&mut self, cx: &mut App) {
        if self.hud.is_none() {
            if self.listening.is_none() {
                self.drain_hud_queue(cx);
            }
            return;
        }
        let expired = self
            .hud
            .as_ref()
            .and_then(|hud| hud.dismiss_at)
            .map(|t| Instant::now() >= t)
            .unwrap_or(false);
        if expired {
            if let Some(hud) = self.hud.take() {
                close_hud_window(hud.handle, cx);
            }
            self.drain_hud_queue(cx);
            return;
        }
        if !self.hud.as_ref().map(|hud| hud.sticky).unwrap_or(false) {
            return;
        }
        let level = if self.debug_wav.is_some() && self.mic_open {
            0.5
        } else {
            self.level
        };
        self.level = 0.0;
        let mut dirty = false;
        if let Some(listening) = &mut self.listening {
            if (listening.level - level).abs() > 0.02 {
                listening.level = level;
                dirty = true;
            }
        }
        if dirty {
            if let (Some(hud), Some(state)) = (&self.hud, self.listening_state()) {
                let handle = hud.handle;
                if let Err(e) = update_hud_window(handle, state, cx) {
                    self.log(&format!("hud update failed: {e:#}"));
                }
            }
        }
    }

    // --- Polls ---

    fn poll_config(&mut self, cx: &mut App) {
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

    fn note_config_error(&mut self, cx: &mut App, msg: &str) {
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

    fn poll_secure(&mut self, _cx: &mut App) {
        if self.last_secure_poll.elapsed() < Duration::from_millis(SECURE_POLL_MS) {
            return;
        }
        self.last_secure_poll = Instant::now();
        self.perm_granted = [
            permissions::microphone() == PermissionState::Granted,
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

    fn restart_tap(&mut self) {
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

    // --- Menu bar chrome ---

    fn refresh_chrome(&mut self) {
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

    fn warning_active(&self) -> bool {
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

#[derive(Debug)]
enum InsertFailure {
    FocusLost,
    Blocked,
}

impl AppState {
    /// Recent holds the last 5 results in memory only (gone on quit): the
    /// popover reads this once it exists; until then it is logged.
    fn push_recent(&mut self, text: String, tag: Option<String>) {
        self.log(&format!(
            "recent{}: {text:?}",
            tag.as_deref()
                .map(|t| format!(" [{t}]"))
                .unwrap_or_default()
        ));
        self.recent.push_back(RecentEntry { text, tag });
        while self.recent.len() > 5 {
            self.recent.pop_front();
        }
    }
}

// --- Audio device helpers (cpal enumeration; capture stays default) --------

fn default_input_missing() -> bool {
    cpal::default_host().default_input_device().is_none()
}

fn default_input_name() -> Option<String> {
    cpal::default_host()
        .default_input_device()
        .and_then(|d| d.name().ok())
}

fn find_input_device(wanted: &str) -> Option<String> {
    let host = cpal::default_host();
    host.input_devices().ok()?.find_map(|device| {
        let name = device.name().ok()?;
        (name == wanted).then_some(name)
    })
}

fn is_bluetooth_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("bluetooth") || lower.contains("airpods") || lower.contains("beats")
}
