//! Popover window: the GPUI window under the status item (`app-ui.md` §4).
//!
//! The status click sends [`MenuAction::TogglePopover`][jisuno_platform::status_item::MenuAction]
//! over the menu channel; the tick drains it here. The window closes on
//! focus loss (polled in the tick), on Esc (the tap path), on a second
//! click, and on intents that leave it (Settings, Quit). Row clicks arrive
//! as [`PopoverIntent`]s over a dedicated channel and are drained the same
//! way, so no `Rc<RefCell<AppState>>` plumbing is needed.

use std::io::Write as _;
use std::process::Stdio;
use std::time::{Duration, Instant};

use gpui::{
    prelude::*, px, size, AnyWindowHandle, App, Bounds, Context, Entity, Point, Window,
    WindowBounds, WindowOptions,
};
use jisuno_platform::permissions::{self, Pane as PlatformPane};
use jisuno_ui::popover_model::{
    ModelPresence, ModelState as PopoverModelState, PermissionPane, Permissions, PopoverIntent,
    PopoverModel, RecentItem,
};
use jisuno_ui::popover_view::{popover_view, PopoverEmit};
use jisuno_ui::tokens::{Theme, POPOVER_W};

use super::model::ModelState as AppModelState;
use super::state::AppState;

/// Toast lifetime after `CopyRecent` (§4: `Copied`).
const TOAST_MS: u64 = 1500;
/// Grace before an un-clicked popover may auto-close (it is never active).
const POPOVER_GRACE_MS: u64 = 1000;

/// The live popover window and the model it last rendered.
pub(crate) struct PopoverHandle {
    pub(crate) window: AnyWindowHandle,
    pub(crate) view: Entity<PopoverRoot>,
    pub(crate) shown_at: Instant,
    pub(crate) was_active: bool,
    pub(crate) last_model: PopoverModel,
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
        let presence = match self.model_state {
            AppModelState::Warm => ModelPresence::Warm,
            AppModelState::Loading { .. } => ModelPresence::Loading,
            AppModelState::Unloaded if self.model_present => ModelPresence::Cold,
            AppModelState::Unloaded | AppModelState::Failed => ModelPresence::NotDownloaded,
        };
        let recent = self
            .recent
            .iter()
            .rev()
            .take(5)
            .map(|entry| {
                let (command, text) = entry
                    .text
                    .strip_prefix("⌘ ")
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
        if self.popover.take().is_some() {
            self.log("popover closed (toggle)");
            return;
        }
        self.open_popover(cx);
    }

    /// Close the popover if it is open (Esc path, focus loss).
    pub(crate) fn close_popover(&mut self, why: &str) {
        if self.popover.take().is_some() {
            self.log(&format!("popover closed ({why})"));
        }
    }

    fn open_popover(&mut self, cx: &mut App) {
        let model = self.popover_model();
        let theme = self.ui_theme(cx);
        let height = popover_height(&model);
        let screen = Self::display_metrics(cx);
        let (mut x, y) = self
            .status
            .button_frame()
            .map(|(bx, _by, bw, _bh)| {
                (
                    (bx + bw - f64::from(POPOVER_W)).clamp(8.0, screen.width - f64::from(POPOVER_W) - 8.0),
                    screen.usable_top + 4.0,
                )
            })
            .unwrap_or(((screen.width - f64::from(POPOVER_W)) / 2.0, screen.usable_top + 4.0));
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
        let theme_open = theme.clone();
        let emit_open = emit.clone();
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
                    shown_at: Instant::now(),
                    was_active: false,
                    last_model: model,
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
            PopoverIntent::DownloadModel => self.log("model download comes later"),
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
            PopoverIntent::OpenSettings => self.log("settings window comes later"),
            PopoverIntent::Quit => self.quit_requested = true,
        }
    }

    /// Refresh the showing popover and close it on focus loss (§4: Esc
    /// closes; a click outside dismisses the transient window).
    pub(crate) fn poll_popover(&mut self, cx: &mut App) {
        if self.popover.is_none() {
            return;
        }
        let model = self.popover_model();
        let theme = self.ui_theme(cx);
        let Some(handle) = self.popover.as_mut() else {
            return;
        };
        let active = handle
            .window
            .update(cx, |_, window, _| window.is_window_active())
            .unwrap_or(false);
        if active {
            handle.was_active = true;
        } else if handle.was_active
            || handle.shown_at.elapsed() > Duration::from_millis(POPOVER_GRACE_MS)
        {
            self.popover = None;
            self.log("popover closed (focus lost)");
            return;
        }
        if model != handle.last_model {
            handle.last_model = model.clone();
            let _ = handle.view.update(cx, |view, cx| {
                view.model = model;
                view.theme = theme;
                cx.notify();
            });
        }
    }
}

/// Copy text to the system clipboard via `pbcopy` (no lingering state).
fn copy_to_clipboard(text: &str) {
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
