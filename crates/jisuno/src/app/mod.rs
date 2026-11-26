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
//! Split into a module dir (step 1): `mod.rs` owns startup (`run`) and the
//! key-mapping helpers; `state` owns the data; `worker`, `pipeline`, `hud`,
//! `config`, `model`, `status` own `impl AppState` method groups. No behavior
//! change from the single-file `app.rs`.

pub(crate) mod command;
pub(crate) mod config;
pub(crate) mod hud;
pub(crate) mod model;
pub(crate) mod pipeline;
pub(crate) mod state;
pub(crate) mod status;
pub(crate) mod worker;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{ActivationPolicy, App, AsyncApp};
use jisuno_asr::setup::Model as AsrModel;
use jisuno_logic::hold::{CapturedKey, HoldKind, HoldMachine, Timing};
use jisuno_logic::keybindings::{self, Command, Key, Keybindings};
use jisuno_logic::settings::{self, ConfigDir, LastGood, Settings};
use jisuno_platform::{
    keys::{Captured, KeyTap, RightKey, ShouldCapture, TapConfig},
    permissions::{self, Pane, PermissionState},
    status_item::StatusItem,
};
use jisuno_ui::hud_model::HudState as UiHud;

use crate::app::config::{embedded_app_files, read_file, stamp};
use crate::app::model::{model_id, ModelState};
use crate::app::state::{AppState, Holder, TICK_MS};
use crate::app::worker::worker_main;

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
        current: keybindings::parse(include_str!(
            "../../../jisuno-logic/defaults/keybindings.json"
        ))
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

    let (voice_config, voice_error) = command::load_voice_config(&dir);
    let installed_apps = crate::runtime::scan_installed_apps(&crate::runtime::applications_dirs());
    let installed_at = Instant::now();
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let (hook_tx, hook_rx) = mpsc::channel();

    let mut startup_errors: Vec<UiHud> = Vec::new();
    if let Some(message) = voice_error {
        stamp(start, &format!("config error: {message}"));
        startup_errors.push(UiHud::ConfigError {
            location: "voice-control-config.json".to_string(),
            message,
        });
    }
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
    // With --debug-wav there is no mic use, so never prompt for it.
    let mic_override = opts.debug_wav.is_some();
    let mut missing: Vec<(Pane, UiHud)> = Vec::new();
    let mic = permissions::microphone();
    if !mic_override && mic == PermissionState::NotDetermined {
        stamp(start, "microphone: not determined — requesting");
        match permissions::request_microphone() {
            Ok(state) => stamp(start, &format!("microphone prompt answered: {state:?}")),
            Err(e) => stamp(start, &format!("microphone prompt failed: {e:#}")),
        }
    }
    if !mic_override && permissions::microphone() != PermissionState::Granted {
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
            mic_satisfied: mic_override,
            bluetooth_now: false,
            missing_device_note: None,
            secure_now: false,
            last_config_error: None,
            debug_wav: opts.debug_wav,
            debug_hold,
            quit_requested: false,
            voice_config: voice_config.clone(),
            installed_apps,
            installed_at,
            pending_plan: None,
            confirming: None,
            esc_cancel: false,
            cmd_tx,
            cmd_rx,
            hook_tx,
            hook_rx,
            settings_good: LastGood::new(settings.clone()),
            bindings_good: LastGood::new(bindings_holder.current.clone()),
            voice_good: LastGood::new(voice_config.clone()),
            theme_good: LastGood::new(theme_holder.current.clone()),
            watch_rx: crate::watch::spawn_watcher(
                vec![
                    dir.settings_path(),
                    dir.keybindings_path(),
                    dir.theme_path(),
                    dir.voice_control_path(),
                ],
                std::time::Duration::from_millis(500),
            ),
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

pub(crate) fn tap_config_for(bindings: &Keybindings) -> TapConfig {
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

pub(crate) fn hold_kind_for(bindings: &Keybindings, key: RightKey) -> Option<HoldKind> {
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
