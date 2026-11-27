//! Popover window: the GPUI window under the status item (`app-ui.md` §4).
//!
//! The status click sends [`MenuAction::TogglePopover`][jisuno_platform::status_item::MenuAction]
//! over the menu channel; the tick drains it here. The window closes on
//! focus loss (polled in the tick), on Esc (the tap path), on a second
//! click, and on intents that leave it (Settings, Quit). Row clicks arrive
//! as [`PopoverIntent`]s over a dedicated channel and are drained the same
//! way, so no `Rc<RefCell<AppState>>` plumbing is needed.
//!
//! The Settings window lives here too (same channel pattern, single
//! instance): every control writes its file atomically and hot reload
//! applies it; the tick refreshes the model from live state.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Write as _;
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use gpui::{
    prelude::*, px, size, AnyWindowHandle, App, Bounds, Context, Entity, Point, TitlebarOptions,
    Window, WindowBounds, WindowOptions,
};
use jisuno_logic::keybindings::{Command as KeyCommand, Key, Keybindings};
use jisuno_logic::settings::{self, ConfigDir};
use jisuno_platform::keys::KeyEvent;
use jisuno_platform::permissions::{self, Pane as PlatformPane};
use jisuno_ui::popover_model::{
    ModelPresence, ModelState as PopoverModelState, PermissionPane, Permissions, PopoverIntent,
    PopoverModel, RecentItem,
};
use jisuno_ui::popover_view::{popover_view, PopoverEmit};
use jisuno_ui::settings_model::{
    command_rows, feed_recorded_key, file_rows, performance_rows, AppearanceModel, AudioModel,
    GeneralModel, KeybindingRow, ModelId, ModelPaneModel, Pane as SettingsPane, PerfRow,
    RecordOutcome, SettingsIntent, SettingsModel, WorkspaceRow,
};
use jisuno_ui::settings_view::{settings_view, SettingsEmit};
use jisuno_ui::tokens::{Theme, POPOVER_W};
use jisuno_ui::welcome_model::{
    DownloadProgress, PermissionKind, WelcomeIntent, WelcomeModel, MODEL_TOTAL_MB,
};
use jisuno_ui::welcome_view::{welcome_view, WelcomeEmit};

use super::model::ModelState as AppModelState;
use super::state::AppState;

/// Toast lifetime after `CopyRecent` (§4: `Copied`).
const TOAST_MS: u64 = 1500;
/// How long the Settings audio picker reuses one device enumeration.
const DEVICE_CACHE_MS: u64 = 5000;
/// Prefix of a Recent entry that is a voice command (not dictated text),
/// shared with [`jisuno_ui::popover_model`]: written when the entry is
/// pushed; read back here for the popover row.
pub(crate) use jisuno_ui::popover_model::COMMAND_RECENT_PREFIX;

/// The live popover window and the model it last rendered.
pub(crate) struct PopoverHandle {
    pub(crate) window: AnyWindowHandle,
    pub(crate) view: Entity<PopoverRoot>,
    pub(crate) was_active: bool,
    pub(crate) last_model: PopoverModel,
    /// [`AppState::popover_fingerprint`] at the last model rebuild.
    pub(crate) fingerprint: u64,
}

/// The GPUI view: a rendered [`PopoverModel`] plus the intent callback.
pub(crate) struct PopoverRoot {
    model: PopoverModel,
    theme: Theme,
    emit: PopoverEmit,
}

impl Render for PopoverRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        popover_view(&self.model, &self.theme, self.emit.clone())
    }
}

impl AppState {
    /// Build the popover model from live state (§4).
    pub(crate) fn popover_model(&self) -> PopoverModel {
        let presence = if let Some(active) = &self.download {
            ModelPresence::Downloading {
                percent: active.percent,
            }
        } else {
            match self.model_state {
                AppModelState::Warm => ModelPresence::Warm,
                AppModelState::Loading { .. } => ModelPresence::Loading,
                AppModelState::Unloaded if self.model_present => ModelPresence::Cold,
                AppModelState::Unloaded | AppModelState::Failed => ModelPresence::NotDownloaded,
            }
        };
        let recent = self
            .recent
            .iter()
            .rev()
            .take(5)
            .map(|entry| {
                let (command, text) = entry
                    .text
                    .strip_prefix(COMMAND_RECENT_PREFIX)
                    .map(|rest| (true, rest.to_string()))
                    .unwrap_or((false, entry.text.clone()));
                RecentItem {
                    text,
                    command,
                    failed: entry.tag.as_deref() == Some("failed"),
                    not_inserted: entry.tag.as_deref() == Some("not inserted"),
                }
            })
            .collect();
        PopoverModel {
            model_name: self.model_name.clone(),
            model: PopoverModelState {
                presence,
                unload_after_idle_min: self.settings.unload_after_idle_min,
            },
            permissions: Permissions {
                microphone: self.perm_granted[0],
                accessibility: self.perm_granted[1],
                input_monitoring: self.perm_granted[2],
            },
            secure_input_holder: self
                .secure_now
                .then(|| {
                    self.announced_secure
                        .last()
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_string())
                })
                .filter(|holder| !holder.is_empty()),
            recent,
            toast: self
                .popover_toast
                .as_ref()
                .filter(|(_, at)| at.elapsed() < Duration::from_millis(TOAST_MS))
                .map(|(toast, _)| toast.clone()),
        }
    }

    /// Digest of every input [`Self::popover_model`] and [`Self::ui_theme`]
    /// read. The tick compares it to the last rebuild and skips the rebuild
    /// when nothing changed.
    pub(crate) fn popover_fingerprint(&self, cx: &App) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.model_name.hash(&mut hasher);
        self.download
            .as_ref()
            .map(|active| active.percent)
            .hash(&mut hasher);
        std::mem::discriminant(&self.model_state).hash(&mut hasher);
        self.model_present.hash(&mut hasher);
        self.settings.unload_after_idle_min.hash(&mut hasher);
        self.perm_granted.hash(&mut hasher);
        self.secure_now.hash(&mut hasher);
        self.announced_secure.last().hash(&mut hasher);
        for entry in self.recent.iter().rev().take(5) {
            entry.text.hash(&mut hasher);
            entry.tag.hash(&mut hasher);
        }
        self.popover_toast
            .as_ref()
            .filter(|(_, at)| at.elapsed() < Duration::from_millis(TOAST_MS))
            .map(|(toast, _)| toast.clone())
            .hash(&mut hasher);
        let theme = self.theme_good.current();
        theme.name.hash(&mut hasher);
        theme.appearance.hash(&mut hasher);
        theme.accent_dictate.hash(&mut hasher);
        theme.accent_command.hash(&mut hasher);
        theme.hud_style.hash(&mut hasher);
        theme.show_latency.hash(&mut hasher);
        matches!(
            cx.window_appearance(),
            gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
        )
        .hash(&mut hasher);
        hasher.finish()
    }

    /// Current theme: `theme.json`, following the system unless `fixed`.
    pub(crate) fn ui_theme(&self, cx: &App) -> Theme {
        let current = self.theme_good.current();
        let value = serde_json::json!({
            "name": current.name,
            "appearance": current.appearance,
            "accent_dictate": current.accent_dictate,
            "accent_command": current.accent_command,
            "hud": { "style": current.hud_style, "show_latency": current.show_latency },
        });
        let system_dark = matches!(
            cx.window_appearance(),
            gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
        );
        Theme::from_theme_json(&value, system_dark).unwrap_or_else(|_| {
            if system_dark {
                Theme::quiet_dark()
            } else {
                Theme::quiet_light()
            }
        })
    }

    /// Toggle the popover: close it if open, else open it under the item.
    pub(crate) fn toggle_popover(&mut self, cx: &mut App) {
        if self.popover.is_some() {
            self.close_popover(cx, "toggle");
            return;
        }
        self.open_popover(cx);
    }

    /// Close the popover if it is open (Esc path, focus loss). Dropping
    /// the handle alone leaves the GPUI window alive (handles are plain
    /// ids), so remove the window first or every cycle leaks a surface.
    pub(crate) fn close_popover(&mut self, cx: &mut App, why: &str) {
        if let Some(handle) = self.popover.take() {
            let _ = handle.window.update(cx, |_, window, _| {
                window.remove_window();
            });
            self.log(&format!("popover closed ({why})"));
        }
    }

    fn open_popover(&mut self, cx: &mut App) {
        let model = self.popover_model();
        let theme = self.ui_theme(cx);
        let height = popover_height(&model);
        let screen = Self::display_metrics(cx);
        // Anchor under the status item like a click: right-aligned to the
        // button frame. When the frame is unavailable (early startup,
        // debug-open before the button has a window), fall back to the
        // right edge under the status area rather than the centre, so the
        // popover still lands where a click would put it.
        let fallback_x = (screen.width - f64::from(POPOVER_W) - 8.0).max(8.0);
        let (mut x, y) = self
            .status
            .button_frame()
            .map(|(bx, _by, bw, _bh)| {
                (
                    (bx + bw - f64::from(POPOVER_W))
                        .clamp(8.0, screen.width - f64::from(POPOVER_W) - 8.0),
                    screen.usable_top + 4.0,
                )
            })
            .unwrap_or((fallback_x, screen.usable_top + 4.0));
        x = x.max(8.0);
        let bounds = Bounds {
            origin: Point {
                x: px(x as f32),
                y: px(y as f32),
            },
            size: size(px(POPOVER_W), px(height)),
        };
        let sender = self.popover_tx.clone();
        let emit: PopoverEmit = std::rc::Rc::new(
            move |intent: PopoverIntent, _window: &mut Window, _cx: &mut App| {
                let _ = sender.send(intent);
            },
        );
        let model_open = model.clone();
        let theme_open = theme;
        let emit_open = emit.clone();
        let fingerprint = self.popover_fingerprint(cx);
        match cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: None,
                focus: true,
                show: true,
                window_background: gpui::WindowBackgroundAppearance::Transparent,
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| PopoverRoot {
                    model: model_open,
                    theme: theme_open,
                    emit: emit_open,
                })
            },
        ) {
            Ok(handle) => {
                self.log(&format!("popover open ({:.0},{:.0})", x, y));
                self.popover = Some(PopoverHandle {
                    window: handle.into(),
                    view: handle_entity(cx, &handle),
                    was_active: false,
                    last_model: model,
                    fingerprint,
                });
            }
            Err(e) => self.log(&format!("popover open failed: {e:#}")),
        }
    }

    /// Drain row clicks from the popover view.
    pub(crate) fn drain_popover(&mut self, cx: &mut App) {
        let mut intents = Vec::new();
        while let Ok(intent) = self.popover_rx.try_recv() {
            intents.push(intent);
        }
        for intent in intents {
            self.log(&format!("popover intent: {intent:?}"));
            self.handle_popover_intent(cx, intent);
        }
    }

    fn handle_popover_intent(&mut self, cx: &mut App, intent: PopoverIntent) {
        match intent {
            PopoverIntent::WarmModel => self.start_load(cx, "popover"),
            PopoverIntent::UnloadModel => self.unload_model("popover"),
            PopoverIntent::DownloadModel => self.start_download(self.model_name.clone()),
            PopoverIntent::CopyRecent(index) => {
                let text = self
                    .popover_model()
                    .recent_rows()
                    .get(index)
                    .map(|item| item.display_text());
                if let Some(text) = text {
                    copy_to_clipboard(&text);
                    self.popover_toast = Some(("Copied".to_string(), Instant::now()));
                }
            }
            PopoverIntent::OpenPermissionPane(pane) => {
                let platform = match pane {
                    PermissionPane::Microphone => PlatformPane::Microphone,
                    PermissionPane::Accessibility => PlatformPane::Accessibility,
                    PermissionPane::InputMonitoring => PlatformPane::InputMonitoring,
                };
                if let Err(e) = permissions::open_settings_pane(platform) {
                    self.log(&format!("open settings pane: {e:#}"));
                }
            }
            PopoverIntent::OpenSettings => self.open_settings(cx),
            PopoverIntent::Quit => self.quit_requested = true,
        }
    }

    /// Refresh the showing popover and close it on focus loss (§4: Esc
    /// closes; a click outside dismisses the transient window).
    pub(crate) fn poll_popover(&mut self, cx: &mut App) {
        if self.popover.is_none() {
            return;
        }
        let fingerprint = self.popover_fingerprint(cx);
        let Some(handle) = self.popover.as_mut() else {
            return;
        };
        let active = handle
            .window
            .update(cx, |_, window, _| window.is_window_active())
            .unwrap_or(false);
        if active {
            handle.was_active = true;
        } else if handle.was_active {
            // Only an observed-active panel closes on focus loss. Closing
            // a never-active panel here raced the open (click, debug-open
            // or voice menu): AppKit briefly reports inactive while the
            // panel activates, which opened and closed the popover by
            // itself. A never-active panel still closes via toggle/Esc.
            self.close_popover(cx, "focus lost");
            return;
        }
        if fingerprint != handle.fingerprint {
            handle.fingerprint = fingerprint;
            let model = self.popover_model();
            let theme = self.ui_theme(cx);
            let Some(handle) = self.popover.as_mut() else {
                return;
            };
            if model != handle.last_model {
                handle.last_model = model.clone();
                handle.view.update(cx, |view, cx| {
                    view.model = model;
                    view.theme = theme;
                    cx.notify();
                });
            }
        }
    }
}

/// Copy text to the system clipboard via `pbcopy` (no lingering state). The
/// child is reaped on a short-lived thread so it never lingers as a zombie.
pub(crate) fn copy_to_clipboard(text: &str) {
    let mut child = match std::process::Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            eprintln!("jisuno app: pbcopy failed: {e:#}");
            return;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

/// Window height for the model: base chrome plus one row per Recent entry.
fn popover_height(model: &PopoverModel) -> f32 {
    let mut height = 360.0;
    height += 26.0 * model.recent_rows().len() as f32;
    if model.secure_input_holder.is_some() {
        height += 26.0;
    }
    if model.toast.is_some() {
        height += 30.0;
    }
    height
}

/// Reborrow the typed entity from the freshly opened window handle.
fn handle_entity(cx: &mut App, handle: &gpui::WindowHandle<PopoverRoot>) -> Entity<PopoverRoot> {
    handle
        .update(cx, |_, _, cx| cx.entity())
        .expect("popover window just opened")
}

/// The live Settings window (single instance) and its intent channel drain.
/// `last` mirrors the view's model so the tick can refresh it from live
/// state without asking the entity.
pub(crate) struct SettingsHandle {
    pub(crate) window: AnyWindowHandle,
    pub(crate) view: Entity<SettingsRoot>,
    pub(crate) last: SettingsModel,
}

impl SettingsHandle {
    fn set_model(&mut self, cx: &mut App, model: SettingsModel, theme: Theme) {
        self.last = model.clone();
        self.view.update(cx, |view, cx| {
            view.theme = theme;
            view.model = model;
            cx.notify();
        });
    }
}

/// The GPUI view: a rendered [`SettingsModel`] plus the intent callback.
pub(crate) struct SettingsRoot {
    model: SettingsModel,
    theme: Theme,
    emit: SettingsEmit,
}

impl Render for SettingsRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        settings_view(&self.model, &self.theme, self.emit.clone())
    }
}

impl AppState {
    /// Open Settings, or bring it to the front if it is already open.
    /// ⌃⌥, and the `jisuno.settings` action land here.
    pub(crate) fn open_settings(&mut self, cx: &mut App) {
        if self.bench_report.is_none() {
            self.bench_report = load_cached_bench();
        }
        if let Some(handle) = &self.settings_win {
            let _ = handle.window.update(cx, |_, window, _| {
                window.activate_window();
            });
            self.log("settings already open (focused)");
            return;
        }
        let model = self.settings_model(None);
        let theme = self.ui_theme(cx);
        let sender = self.settings_tx.clone();
        let emit: SettingsEmit = std::rc::Rc::new(
            move |intent: SettingsIntent, _window: &mut Window, _cx: &mut App| {
                let _ = sender.send(intent);
            },
        );
        let model_open = model.clone();
        let emit_open = emit.clone();
        let screen = Self::display_metrics(cx);
        let win_w = 800.0f32;
        let win_h = 520.0f32;
        let win_bounds = Bounds {
            origin: Point {
                x: px(((screen.width - f64::from(win_w)) / 2.0).max(0.0) as f32),
                y: px((screen.usable_top
                    + ((screen.height - screen.usable_top - f64::from(win_h)) / 2.0))
                    .max(screen.usable_top) as f32),
            },
            size: size(px(win_w), px(win_h)),
        };
        match cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(win_bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Settings".into()),
                    ..Default::default()
                }),
                focus: true,
                show: true,
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| SettingsRoot {
                    model: model_open,
                    theme,
                    emit: emit_open,
                })
            },
        ) {
            Ok(handle) => {
                self.log("settings open");
                self.settings_win = Some(SettingsHandle {
                    window: handle.into(),
                    last: model.clone(),
                    view: handle
                        .update(cx, |_, _, cx| cx.entity())
                        .expect("settings window just opened"),
                });
            }
            Err(e) => self.log(&format!("settings open failed: {e:#}")),
        }
    }

    /// Build the [`SettingsModel`] from live state. `keep` carries the parts
    /// the host never rebuilds: the selected pane and any in-progress
    /// keybinding recording, conflict, error or warning.
    pub(crate) fn settings_model(&self, keep: Option<&SettingsModel>) -> SettingsModel {
        let s = &self.settings;
        let t = self.theme_good.current();
        let devices = audio_input_devices();
        let selected = match s.model {
            settings::Model::ParakeetV3Int8 => ModelId::V3,
            settings::Model::ParakeetV2Int8 => ModelId::V2,
        };
        let selected_file_id = selected.file_id();
        let downloaded = jisuno_asr::setup::Model::parse(selected_file_id)
            .map(|m| m.is_ready())
            .unwrap_or(false);
        let (warm, loading) = match self.model_state {
            AppModelState::Warm => (self.model_name == selected_file_id, false),
            AppModelState::Loading { .. } => (false, self.model_name == selected_file_id),
            _ => (false, false),
        };
        let download_percent = self
            .download
            .as_ref()
            .filter(|active| active.model_id == selected_file_id)
            .map(|active| active.percent);
        let (builtin_commands, agent_commands) = command_rows(&self.voice_config);
        let workspaces: Vec<WorkspaceRow> = self
            .voice_config
            .workspaces
            .iter()
            .flat_map(|(name, windows)| windows.iter().map(|window| WorkspaceRow::of(name, window)))
            .collect();
        let dir = ConfigDir::resolve()
            .map(|dir| dir.root().display().to_string())
            .unwrap_or_else(|_| "~/.config/jisuno".to_string());
        let file_errors: Vec<(&'static str, String)> = self
            .last_config_error
            .as_ref()
            .and_then(|message| {
                let (file, _) = message.split_once(':')?;
                let file: &'static str = match file {
                    "settings.json" => "settings.json",
                    "keybindings.json" => "keybindings.json",
                    "theme.json" => "theme.json",
                    "voice-control-config.json" => "voice-control-config.json",
                    _ => return None,
                };
                Some((file, message.clone()))
            })
            .into_iter()
            .collect();
        let keep_binding_ui = keep.filter(|_| self.key_capturing.is_some());
        SettingsModel {
            pane: keep
                .map(|model| model.pane)
                .unwrap_or(SettingsPane::General),
            general: GeneralModel {
                hud: match s.hud {
                    settings::Hud::Notch => "notch".into(),
                    settings::Hud::Cursor => "cursor".into(),
                    settings::Hud::Off => "off".into(),
                },
                sounds: s.sounds,
                partials: s.partials,
                insert: match s.insert {
                    settings::Insert::Paste => "paste".into(),
                    settings::Insert::Type => "type".into(),
                },
                insert_type_apps: s.insert_type_apps.clone(),
                restore_clipboard: s.restore_clipboard,
                dictation_limit_s: s.dictation_limit_s,
                hands_free_silence_ms: s.hands_free_silence_ms,
                login_item: s.login_item,
            },
            audio: AudioModel {
                input_device: if devices.iter().any(|device| device == &s.input_device) {
                    s.input_device.clone()
                } else if s.input_device == "system" {
                    "system".into()
                } else {
                    format!("{} (not connected)", s.input_device)
                },
                devices,
                testing_mic: false,
            },
            model: ModelPaneModel {
                selected,
                downloaded,
                warm,
                loading,
                download_percent,
                load_policy: match s.model_load {
                    settings::ModelLoad::OnDemand => "on_demand".into(),
                    settings::ModelLoad::AtLogin => "at_login".into(),
                },
                unload_after_idle_min: s.unload_after_idle_min,
            },
            keybinding_rows: keep_binding_ui
                .map(|model| model.keybinding_rows.clone())
                .unwrap_or_else(|| KeybindingRow::all_from(self.bindings_good.current())),
            recording: keep_binding_ui.and_then(|model| model.recording.clone()),
            pending_conflict: keep_binding_ui.and_then(|model| model.pending_conflict.clone()),
            keybinding_error: keep_binding_ui.and_then(|model| model.keybinding_error.clone()),
            keybinding_warning: keep_binding_ui.and_then(|model| model.keybinding_warning.clone()),
            builtin_commands,
            agent_commands,
            workspaces,
            appearance: AppearanceModel {
                name: t.name.clone(),
                appearance: t.appearance.clone(),
                accent_dictate: t.accent_dictate.clone(),
                accent_command: t.accent_command.clone(),
                hud_style: t.hud_style.clone(),
                show_latency: t.show_latency,
            },
            perf: self.bench_perf_rows(),
            bench_running: self.bench_running,
            files: file_rows(&dir, &file_errors),
        }
    }

    /// Drain Settings row clicks.
    pub(crate) fn drain_settings(&mut self, cx: &mut App) {
        let mut intents = Vec::new();
        while let Ok(intent) = self.settings_rx.try_recv() {
            intents.push(intent);
        }
        for intent in intents {
            self.log(&format!("settings intent: {intent:?}"));
            self.handle_settings_intent(cx, intent);
        }
    }

    fn handle_settings_intent(&mut self, cx: &mut App, intent: SettingsIntent) {
        match intent {
            SettingsIntent::SelectPane(pane) => self.update_settings_model(cx, |model| {
                model.pane = pane;
            }),
            SettingsIntent::SetSetting { file, key, value } => {
                self.write_setting(cx, file, &key, value);
            }
            SettingsIntent::StartRecording { command, when } => {
                self.update_settings_model(cx, |model| {
                    model.recording = Some(jisuno_ui::settings_model::Recording {
                        command,
                        when,
                        live_display: "Press keys…".into(),
                    });
                    model.pending_conflict = None;
                    model.keybinding_error = None;
                    model.keybinding_warning = None;
                });
                self.key_capturing = Some((command, when));
                self.tap_capture.store(true, Ordering::SeqCst);
                self.restart_tap();
                self.log("keybinding recording started");
            }
            SettingsIntent::CancelRecording => {
                self.end_capture();
                self.update_settings_model(cx, |model| {
                    model.recording = None;
                    model.pending_conflict = None;
                });
            }
            SettingsIntent::ReplaceBinding => {
                self.end_capture();
                self.update_settings_model(cx, apply_pending_replace);
                self.persist_keybindings(cx);
            }
            SettingsIntent::ResetBinding(command) => {
                self.reset_keybinding_to_default(cx, command);
                self.persist_keybindings(cx);
            }
            SettingsIntent::OpenFile(name) => self.open_config_file(name),
            SettingsIntent::CopyError(name) => {
                let message = self
                    .last_config_error
                    .clone()
                    .filter(|message| message.starts_with(name));
                if let Some(message) = message {
                    copy_to_clipboard(&message);
                }
            }
            SettingsIntent::DownloadModel(file_id) => self.start_download(file_id.to_string()),
            SettingsIntent::RemoveModel(file_id) => self.remove_model(file_id),
            SettingsIntent::WarmModel => self.start_load(cx, "settings"),
            SettingsIntent::UnloadModel => self.unload_model("settings"),
            SettingsIntent::TestMic => self.log("mic level meter comes later"),
            SettingsIntent::RunBench => self.run_bench(),
        }
    }

    /// Leave capture mode: normal tap config, keys flow to apps again.
    fn end_capture(&mut self) {
        if self.key_capturing.is_some() {
            self.log("keybinding recording finished");
        }
        self.key_capturing = None;
        self.tap_capture.store(false, Ordering::SeqCst);
        self.restart_tap();
    }

    /// Feed one captured tap event into the in-progress recording (§9).
    /// A saved binding is persisted at once (hot reload applies it); a
    /// conflict or error shows inline; Esc cancels and keeps the old bind.
    pub(crate) fn feed_capture(&mut self, cx: &mut App, event: &KeyEvent) {
        if self.settings_win.is_none() {
            self.end_capture();
            return;
        }
        let key = match event {
            KeyEvent::Chord(key) => Some(key.clone()),
            KeyEvent::HoldDown(side) => Some(Key::Hold((*side).into())),
            KeyEvent::Escape => Key::parse("escape").ok(),
            KeyEvent::Return => Key::parse("return").ok(),
            _ => None,
        };
        let Some(key) = key else {
            return;
        };
        let Some(mut model) = self.settings_win.as_ref().map(|handle| handle.last.clone()) else {
            self.end_capture();
            return;
        };
        self.log(&format!("keybinding recorded: {}", key.file()));
        let outcome = feed_recorded_key(&mut model, key);
        let finished = model.recording.is_none();
        let saved = matches!(outcome, RecordOutcome::Saved { .. });
        let theme = self.ui_theme(cx);
        if let Some(handle) = self.settings_win.as_mut() {
            handle.set_model(cx, model, theme);
        }
        if finished {
            self.end_capture();
        }
        if saved {
            self.persist_keybindings(cx);
        }
    }

    /// Apply `f` to the showing Settings model and notify the view.
    fn update_settings_model(&mut self, cx: &mut App, f: impl FnOnce(&mut SettingsModel)) {
        let Some(last) = self.settings_win.as_ref().map(|handle| handle.last.clone()) else {
            return;
        };
        let mut model = last;
        f(&mut model);
        let theme = self.ui_theme(cx);
        if let Some(handle) = self.settings_win.as_mut() {
            handle.set_model(cx, model, theme);
        }
    }

    /// Write one control to its file atomically; hot reload applies it. A
    /// failed write keeps the last good file and shows the message.
    fn write_setting(
        &mut self,
        cx: &mut App,
        file: &'static str,
        key: &str,
        value: serde_json::Value,
    ) {
        let path = match ConfigDir::resolve().map(|dir| match file {
            "settings" => dir.settings_path(),
            "theme" => dir.theme_path(),
            _ => dir.settings_path(),
        }) {
            Ok(path) => path,
            Err(e) => {
                self.note_config_error(cx, &format!("{file}: {e:#} · using last good config"));
                return;
            }
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                self.note_config_error(cx, &format!("{file}: {e:#} · using last good config"));
                return;
            }
        };
        let mut doc: serde_json::Value = match serde_json::from_str(&text) {
            Ok(doc) => doc,
            Err(e) => {
                self.note_config_error(cx, &format!("{file}: {e:#} · using last good config"));
                return;
            }
        };
        if let Err(message) = set_json_key(&mut doc, key, value) {
            self.note_config_error(cx, &format!("{file}: {message} · using last good config"));
            return;
        }
        let pretty = serde_json::to_string_pretty(&doc)
            .map(|text| format!("{text}\n"))
            .unwrap_or(text);
        if let Err(e) = settings::write_atomic(&path, &pretty) {
            self.note_config_error(cx, &format!("{file}: {e:#} · using last good config"));
            return;
        }
        self.log(&format!("settings wrote {file} {key}"));
    }

    /// Remove an installed model file (refuses while downloading).
    fn remove_model(&mut self, file_id: &str) {
        if self
            .download
            .as_ref()
            .is_some_and(|active| active.model_id == file_id)
        {
            self.log(&format!(
                "remove {file_id}: download running, cancel it first"
            ));
            return;
        }
        match jisuno_asr::setup::Model::parse(file_id) {
            Ok(model) => match jisuno_asr::setup::remove(&model) {
                Ok(()) => {
                    self.model_present = self.model_files_present();
                    self.log(&format!("remove {file_id}: removed"));
                }
                Err(e) => self.log(&format!("remove {file_id}: {e:#}")),
            },
            Err(e) => self.log(&format!("remove {file_id}: {e:#}")),
        }
    }

    /// Open one config file in its default editor.
    fn open_config_file(&self, name: &'static str) {
        let path = ConfigDir::resolve().map(|dir| match name {
            "settings.json" => dir.settings_path(),
            "keybindings.json" => dir.keybindings_path(),
            "theme.json" => dir.theme_path(),
            "voice-control-config.json" => dir.voice_control_path(),
            _ => dir.settings_path(),
        });
        match path {
            Ok(path) => {
                if let Err(e) = std::process::Command::new("open").arg(&path).status() {
                    self.log(&format!("open {name}: {e:#}"));
                }
            }
            Err(e) => self.log(&format!("open {name}: {e:#}")),
        }
    }

    /// Write the Keybindings pane rows back to `keybindings.json`
    /// atomically; hot reload applies them.
    fn persist_keybindings(&mut self, cx: &mut App) {
        let Some(handle) = &self.settings_win else {
            return;
        };
        let text = keybindings_file_text(&handle.last.bindings());
        let path = ConfigDir::resolve().map(|dir| dir.keybindings_path());
        match path {
            Ok(path) => {
                if let Err(e) = settings::write_atomic(&path, &text) {
                    self.note_config_error(
                        cx,
                        &format!("keybindings.json: {e:#} · using last good config"),
                    );
                } else {
                    self.log("keybindings.json written from Settings");
                    self.restart_tap();
                }
            }
            Err(e) => self.note_config_error(
                cx,
                &format!("keybindings.json: {e:#} · using last good config"),
            ),
        }
    }

    /// `Set to default` for one command, with Replace semantics: clashing
    /// rows on other commands go `Not set`. Commands without a default
    /// binding (`model.unload`, `jisuno.menu`) end up unbound.
    fn reset_keybinding_to_default(&mut self, cx: &mut App, command: KeyCommand) {
        let defaults = jisuno_logic::keybindings::defaults();
        let wanted: Vec<jisuno_logic::keybindings::Binding> = defaults
            .bindings
            .into_iter()
            .filter(|binding| binding.command == command)
            .collect();
        self.update_settings_model(cx, |model| {
            for binding in &wanted {
                model.keybinding_rows.retain(|row| {
                    !(row.command != binding.command
                        && row.key.as_ref() == Some(&binding.key)
                        && row.when == binding.when)
                });
            }
            model.keybinding_rows.retain(|row| row.command != command);
            for binding in wanted.clone() {
                model.keybinding_rows.push(KeybindingRow {
                    command: binding.command,
                    key: Some(binding.key),
                    when: binding.when,
                    hold: binding.hold,
                });
            }
            model.recording = None;
            model.pending_conflict = None;
            model.keybinding_error = None;
        });
    }

    /// Refresh the showing Settings window from live state (download
    /// progress, permission-free rows, hot-reloaded files). The selected
    /// pane and any in-progress recording survive.
    pub(crate) fn poll_settings(&mut self, cx: &mut App) {
        let Some(window) = self.settings_win.as_ref().map(|handle| handle.window) else {
            return;
        };
        // Closed at the window chrome: the handle is dead.
        if window.update(cx, |_, _, _| ()).is_err() {
            self.settings_win = None;
            self.end_capture();
            self.log("settings closed");
            return;
        }
        let keep = self.settings_win.as_ref().map(|handle| handle.last.clone());
        let fresh = self.settings_model(keep.as_ref());
        if Some(&fresh) != keep.as_ref() {
            let theme = self.ui_theme(cx);
            if let Some(handle) = self.settings_win.as_mut() {
                handle.set_model(cx, fresh, theme);
            }
        }
    }
}

/// Apply a shown conflict: the other command loses the key (Replace, §9).
fn apply_pending_replace(model: &mut SettingsModel) {
    let (Some(recording), Some(pending)) =
        (model.recording.clone(), model.pending_conflict.clone())
    else {
        return;
    };
    model.keybinding_rows.retain(|row| {
        !(row.command != recording.command
            && row.key.as_ref() == Some(&pending.key)
            && row.when == pending.when)
    });
    if let Some(row) = model
        .keybinding_rows
        .iter_mut()
        .find(|row| row.command == recording.command && row.when == recording.when)
    {
        row.key = Some(pending.key.clone());
        row.hold = matches!(pending.key, jisuno_logic::keybindings::Key::Hold(_));
    }
    model.recording = None;
    model.pending_conflict = None;
    model.keybinding_error = None;
}

/// Serialize [`Keybindings`] in the `keybindings.json` shape (`§2`).
fn keybindings_file_text(bindings: &Keybindings) -> String {
    let rows: Vec<serde_json::Value> = bindings
        .bindings
        .iter()
        .map(|binding| {
            let mut row =
                serde_json::json!({"key": binding.key.file(), "command": binding.command.id()});
            if binding.hold {
                row["hold"] = true.into();
            }
            if let Some(when) = binding.when {
                row["when"] = when.as_str().into();
            }
            row
        })
        .collect();
    let doc = serde_json::json!({
        "$schema": "./schemas/keybindings.schema.json",
        "bindings": rows,
    });
    format!(
        "{}\n",
        serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".into())
    )
}

/// Installed input devices, system input first (Audio pane picker).
fn audio_input_devices() -> Vec<String> {
    // The Settings tick rebuilds the model every 20 ms; enumeration is slow,
    // so reuse one result for DEVICE_CACHE_MS.
    static CACHE: Mutex<Option<(Instant, Vec<String>)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, devices)) = cache.as_ref() {
        if at.elapsed() < Duration::from_millis(DEVICE_CACHE_MS) {
            return devices.clone();
        }
    }
    let devices = enumerate_audio_inputs();
    *cache = Some((Instant::now(), devices.clone()));
    devices
}

fn enumerate_audio_inputs() -> Vec<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let mut devices = vec!["system".to_string()];
    if let Ok(list) = cpal::default_host().input_devices() {
        for device in list {
            if let Ok(name) = device.name() {
                if !devices.contains(&name) {
                    devices.push(name);
                }
            }
        }
    }
    devices
}

/// The live Welcome window (first run) and its intent channel drain.
/// `last` mirrors the view's model so the tick can refresh it from live
/// state (permissions re-check every second via `poll_secure`).
pub(crate) struct WelcomeHandle {
    pub(crate) window: AnyWindowHandle,
    pub(crate) view: Entity<WelcomeRoot>,
    pub(crate) last: WelcomeModel,
}

impl WelcomeHandle {
    fn set_model(&mut self, cx: &mut App, model: WelcomeModel, theme: Theme) {
        self.last = model.clone();
        self.view.update(cx, |view, cx| {
            view.theme = theme;
            view.model = model;
            cx.notify();
        });
    }
}

/// The GPUI view: a rendered [`WelcomeModel`] plus the intent callback.
pub(crate) struct WelcomeRoot {
    model: WelcomeModel,
    theme: Theme,
    emit: WelcomeEmit,
}

impl Render for WelcomeRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        welcome_view(&self.model, &self.theme, self.emit.clone())
    }
}

impl AppState {
    /// Open Welcome, or bring it to the front if it is already open.
    /// First run opens it automatically; the fallback menu re-opens it.
    pub(crate) fn open_welcome(&mut self, cx: &mut App) {
        if let Some(handle) = &self.welcome_win {
            let _ = handle.window.update(cx, |_, window, _| {
                window.activate_window();
            });
            self.log("welcome already open (focused)");
            return;
        }
        let model = self.welcome_model();
        let theme = self.ui_theme(cx);
        let sender = self.welcome_tx.clone();
        let emit: WelcomeEmit = std::rc::Rc::new(
            move |intent: WelcomeIntent, _window: &mut Window, _cx: &mut App| {
                let _ = sender.send(intent);
            },
        );
        let model_open = model.clone();
        let emit_open = emit.clone();
        let screen = Self::display_metrics(cx);
        let win_w = 540.0f32;
        let win_h = 580.0f32;
        let win_bounds = Bounds {
            origin: Point {
                x: px(((screen.width - f64::from(win_w)) / 2.0).max(0.0) as f32),
                y: px((screen.usable_top
                    + ((screen.height - screen.usable_top - f64::from(win_h)) / 2.0))
                    .max(screen.usable_top) as f32),
            },
            size: size(px(win_w), px(win_h)),
        };
        match cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(win_bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Welcome to Jisuno".into()),
                    ..Default::default()
                }),
                focus: true,
                show: true,
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| WelcomeRoot {
                    model: model_open,
                    theme,
                    emit: emit_open,
                })
            },
        ) {
            Ok(handle) => {
                self.log("welcome open");
                self.welcome_win = Some(WelcomeHandle {
                    window: handle.into(),
                    view: handle
                        .update(cx, |_, _, cx| cx.entity())
                        .expect("welcome window just opened"),
                    last: model,
                });
            }
            Err(e) => self.log(&format!("welcome open failed: {e:#}")),
        }
    }

    /// Build the [`WelcomeModel`] from live state (§6).
    pub(crate) fn welcome_model(&self) -> WelcomeModel {
        WelcomeModel {
            allowed: self.perm_granted,
            files_written: true,
            config_path: Self::config_display(),
            model_present: self.model_present,
            download: self.download.as_ref().map(|active| DownloadProgress {
                percent: active.percent,
                downloaded_mb: active.percent * MODEL_TOTAL_MB / 100,
                total_mb: MODEL_TOTAL_MB,
            }),
        }
    }

    /// Display form of the config dir, with `~` for `$HOME` and a
    /// trailing slash (e.g. `~/.config/jisuno/`).
    fn config_display() -> String {
        let root = ConfigDir::resolve().map(|d| d.root().to_path_buf());
        let mut text = match root {
            Ok(d) => {
                let under_home = std::env::var_os("HOME")
                    .and_then(|h| d.strip_prefix(h).ok().map(|r| r.to_path_buf()));
                match under_home {
                    Some(rel) => format!("~/{}", rel.display()),
                    None => d.display().to_string(),
                }
            }
            Err(_) => "~/.config/jisuno".to_string(),
        };
        if !text.ends_with('/') {
            text.push('/');
        }
        text
    }

    /// Drain Welcome button clicks.
    pub(crate) fn drain_welcome(&mut self, cx: &mut App) {
        let mut intents = Vec::new();
        while let Ok(intent) = self.welcome_rx.try_recv() {
            intents.push(intent);
        }
        for intent in intents {
            self.log(&format!("welcome intent: {intent:?}"));
            self.handle_welcome_intent(cx, intent);
        }
    }

    fn handle_welcome_intent(&mut self, cx: &mut App, intent: WelcomeIntent) {
        match intent {
            WelcomeIntent::OpenPermissionPane(kind) => {
                let platform = match kind {
                    PermissionKind::Microphone => PlatformPane::Microphone,
                    PermissionKind::Accessibility => PlatformPane::Accessibility,
                    PermissionKind::InputMonitoring => PlatformPane::InputMonitoring,
                };
                if let Err(e) = permissions::open_settings_pane(platform) {
                    self.log(&format!("open settings pane: {e:#}"));
                }
            }
            WelcomeIntent::DownloadModel => self.start_download(self.model_name.clone()),
            WelcomeIntent::Start => {
                // Remove the window, not just the handle: a dropped
                // handle leaks the live window (see close_popover).
                if let Some(handle) = self.welcome_win.take() {
                    let _ = handle.window.update(cx, |_, window, _| {
                        window.remove_window();
                    });
                }
                self.log("welcome dismissed (Start)");
            }
        }
    }

    /// Refresh the showing Welcome window (live permissions, download
    /// progress). Closed at the window chrome: the handle is dead.
    pub(crate) fn poll_welcome(&mut self, cx: &mut App) {
        let Some(window) = self.welcome_win.as_ref().map(|handle| handle.window) else {
            return;
        };
        if window.update(cx, |_, _, _| ()).is_err() {
            self.welcome_win = None;
            self.log("welcome closed");
            return;
        }
        let fresh = self.welcome_model();
        let stale = self.welcome_win.as_ref().map(|handle| handle.last.clone());
        if Some(&fresh) != stale.as_ref() {
            let theme = self.ui_theme(cx);
            if let Some(handle) = self.welcome_win.as_mut() {
                handle.set_model(cx, fresh, theme);
            }
        }
    }
}

/// Parsed bench numbers for the Performance pane (§5.2). `None` fields
/// show `not measured`; gated rows carry `· PASS` / `· FAIL`.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct BenchValues {
    pub(crate) keyup_5s_ms: Option<f64>,
    pub(crate) flush_ms: Option<f64>,
    pub(crate) decode_5s_ms: Option<f64>,
    pub(crate) decode_2s_ms: Option<f64>,
    pub(crate) decode_15s_ms: Option<f64>,
    pub(crate) cold_load_ms: Option<f64>,
    pub(crate) insert_ms: Option<f64>,
    pub(crate) rss_warm_bytes: Option<u64>,
    pub(crate) rss_unloaded_bytes: Option<u64>,
    pub(crate) wer_mean: Option<f64>,
}

/// Completion reports from the `jisuno-bench` child process.
pub(crate) enum BenchEvent {
    Done(String),
    Failed(String),
}

fn bench_cache_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .map(|home| std::path::PathBuf::from(home).join("Library/Caches/jisuno/bench"))
}

/// Read the newest `*.json` bench report from the cache dir, if any.
pub(crate) fn load_cached_bench() -> Option<BenchValues> {
    let dir = bench_cache_dir()?;
    let mut newest: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let modified = entry.metadata().and_then(|meta| meta.modified()).ok()?;
        let replace = newest.as_ref().is_none_or(|(mtime, _)| modified > *mtime);
        if replace {
            newest = Some((modified, path));
        }
    }
    let (_, path) = newest?;
    parse_bench_report(&std::fs::read_to_string(path).ok()?)
}

fn parse_bench_report(text: &str) -> Option<BenchValues> {
    let doc: serde_json::Value = serde_json::from_str(text).ok()?;
    let median = |keys: &[&str]| -> Option<f64> {
        let mut node = &doc;
        for key in keys {
            node = node.get(*key)?;
        }
        node.get("median_ms")?.as_f64()
    };
    let bytes = |keys: &[&str]| -> Option<u64> {
        let mut node = &doc;
        for key in keys {
            node = node.get(*key)?;
        }
        node.as_u64()
    };
    Some(BenchValues {
        keyup_5s_ms: median(&["keyup_5s_ms"]),
        flush_ms: median(&["flush_ms"]),
        decode_5s_ms: median(&["decode_5s_ms"]),
        decode_2s_ms: median(&["decode_2s_ms"]),
        decode_15s_ms: median(&["decode_15s_ms"]),
        cold_load_ms: median(&["cold_load_ms"]),
        insert_ms: median(&["insert_ms"]),
        rss_warm_bytes: bytes(&["memory", "rss_warm_bytes"]),
        rss_unloaded_bytes: bytes(&["memory", "rss_unloaded_bytes"]),
        wer_mean: doc.get("wer_mean").and_then(|value| value.as_f64()),
    })
}

fn pass_fail(value: f64, target: f64) -> &'static str {
    if value <= target {
        "PASS"
    } else {
        "FAIL"
    }
}

fn ms_row(value: Option<f64>, target_ms: f64) -> Option<String> {
    value.map(|v| format!("{v:.0} ms · {}", pass_fail(v, target_ms)))
}

impl AppState {
    /// The Performance table: cached/measured numbers with PASS/FAIL on
    /// the gated rows (§5.2, targets from `jisuno.md` §6/§6.1).
    pub(crate) fn bench_perf_rows(&self) -> Vec<PerfRow> {
        let report = self.bench_report.clone().unwrap_or_default();
        let mut measured: [Option<String>; 12] = Default::default();
        measured[0] = ms_row(report.keyup_5s_ms, 150.0);
        measured[1] = ms_row(report.flush_ms, 15.0);
        measured[2] = ms_row(report.decode_5s_ms, 100.0);
        measured[4] = ms_row(report.insert_ms, 10.0);
        measured[6] = match (report.decode_2s_ms, report.decode_15s_ms) {
            (Some(two), Some(fifteen)) => Some(format!("{two:.0} / {fifteen:.0} ms")),
            (Some(two), None) => Some(format!("{two:.0} ms (2 s)")),
            (None, Some(fifteen)) => Some(format!("{fifteen:.0} ms (15 s)")),
            (None, None) => None,
        };
        measured[7] = report
            .cold_load_ms
            .map(|v| format!("{:.2} s · {}", v / 1000.0, pass_fail(v, 1500.0)));
        measured[8] = report.rss_warm_bytes.map(|bytes| {
            let gb = bytes as f64 / 1e9;
            format!("{gb:.2} GB · {}", pass_fail(bytes as f64, 1.2e9))
        });
        measured[9] = report.rss_unloaded_bytes.map(|bytes| {
            let mb = bytes as f64 / 1e6;
            format!("{mb:.0} MB · {}", pass_fail(bytes as f64, 60e6))
        });
        measured[11] = report.wer_mean.map(|mean| format!("{mean:.3}"));
        if self.bench_report.is_none() {
            if let Some(error) = &self.bench_error {
                measured[0] = Some(error.clone());
            }
        }
        performance_rows(&measured)
    }

    /// Run `jisuno-bench --json` (the sibling binary) on a thread; the
    /// JSON lands in the cache dir and the pane picks it up. Without a
    /// sibling binary the pane says how to measure instead.
    pub(crate) fn run_bench(&mut self) {
        if self.bench_running {
            self.log("benchmark already running");
            return;
        }
        self.bench_running = true;
        self.bench_error = None;
        let tx = self.bench_tx.clone();
        let model = self.model_name.clone();
        std::thread::spawn(move || {
            let sibling = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|dir| dir.join("jisuno-bench")))
                .filter(|path| path.is_file());
            let Some(path) = sibling else {
                let _ = tx.send(BenchEvent::Failed(
                    "Run `jisuno-bench --json` to measure".into(),
                ));
                return;
            };
            let output = std::process::Command::new(&path)
                .arg("--json")
                .arg("--model")
                .arg(&model)
                .output();
            match output {
                Ok(out) if out.status.success() => {
                    let text = String::from_utf8_lossy(&out.stdout).into_owned();
                    if serde_json::from_str::<serde_json::Value>(&text).is_err() {
                        let _ = tx.send(BenchEvent::Failed("benchmark returned bad JSON".into()));
                        return;
                    }
                    if let Some(dir) = bench_cache_dir() {
                        let _ = std::fs::create_dir_all(&dir);
                        let epoch = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        let _ = std::fs::write(dir.join(format!("{epoch}.json")), &text);
                    }
                    let _ = tx.send(BenchEvent::Done(text));
                }
                Ok(out) => {
                    let tail = String::from_utf8_lossy(&out.stderr)
                        .lines()
                        .last()
                        .unwrap_or("benchmark failed")
                        .to_string();
                    let _ = tx.send(BenchEvent::Failed(tail));
                }
                Err(e) => {
                    let _ = tx.send(BenchEvent::Failed(format!("benchmark failed: {e:#}")));
                }
            }
        });
    }

    /// Drain finished benchmark runs into the Performance pane state.
    pub(crate) fn drain_bench(&mut self) {
        let mut events = Vec::new();
        while let Ok(event) = self.bench_rx.try_recv() {
            events.push(event);
        }
        for event in events {
            match event {
                BenchEvent::Done(text) => {
                    self.bench_running = false;
                    match parse_bench_report(&text) {
                        Some(values) => {
                            self.bench_report = Some(values);
                            self.bench_error = None;
                            self.log("benchmark finished");
                        }
                        None => {
                            self.bench_error = Some("benchmark returned bad JSON".into());
                            self.log("benchmark returned bad JSON");
                        }
                    }
                }
                BenchEvent::Failed(error) => {
                    self.bench_running = false;
                    self.bench_error = Some(error.clone());
                    self.log(&format!("benchmark: {error}"));
                }
            }
        }
    }
}

/// Set a (possibly dotted, e.g. `hud.show_latency`) key on a JSON document.
/// Missing or `null` parents are created as objects; a parent that is any
/// other non-object value is an error, and nothing is written.
fn set_json_key(
    doc: &mut serde_json::Value,
    key: &str,
    value: serde_json::Value,
) -> Result<(), String> {
    let mut node = doc;
    let mut parts = key.split('.').peekable();
    while let Some(part) = parts.next() {
        let object = node
            .as_object_mut()
            .ok_or_else(|| format!("`{key}`: `{part}` is not inside a JSON object"))?;
        if parts.peek().is_none() {
            object.insert(part.to_string(), value);
            return Ok(());
        }
        node = object.entry(part).or_insert_with(|| serde_json::json!({}));
        if node.is_null() {
            *node = serde_json::json!({});
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::set_json_key;
    use serde_json::json;

    #[test]
    fn sets_top_and_dotted_keys_creating_parents() {
        let mut doc = json!({ "a": 1 });
        set_json_key(&mut doc, "a", json!(2)).unwrap();
        set_json_key(&mut doc, "hud.show_latency", json!(true)).unwrap();
        set_json_key(&mut doc, "hud.style", json!("x")).unwrap();
        assert_eq!(
            doc,
            json!({ "a": 2, "hud": { "show_latency": true, "style": "x" } })
        );
    }

    #[test]
    fn null_parent_becomes_object() {
        let mut doc = json!({ "hud": null });
        set_json_key(&mut doc, "hud.style", json!("x")).unwrap();
        assert_eq!(doc, json!({ "hud": { "style": "x" } }));
    }

    #[test]
    fn non_object_roots_and_parents_are_errors() {
        let mut root = json!([1, 2]);
        assert!(set_json_key(&mut root, "a", json!(1)).is_err());
        assert_eq!(root, json!([1, 2]));

        let mut parent = json!({ "hud": 5 });
        assert!(set_json_key(&mut parent, "hud.style", json!("x")).is_err());
        assert_eq!(parent, json!({ "hud": 5 }));
    }
}
