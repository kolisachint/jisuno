//! HUD windows: listening/decoding/ephemeral show, hide, level refresh.
//!
//! Split from `app.rs` with no behavior change (step 1).

use std::time::{Duration, Instant};

use gpui::App;
use jisuno_logic::settings;
use jisuno_platform::hud_placement::{hud_origin, mouse_position_gpui, HudSize, Screen};
use jisuno_ui::hud_model::{render_model, HudState as UiHud, Mode as UiMode};
use jisuno_ui::hud_window::{
    close_hud_window, open_hud_window, update_hud_window, HUD_WINDOW_SIZE,
};

use super::state::{AppState, HudWindow};

// --- HUD ---

impl AppState {
    // --- HUD ---

    pub(crate) fn hud_size() -> HudSize {
        let (w, h) = HUD_WINDOW_SIZE;
        HudSize {
            width: f64::from(w),
            height: f64::from(h),
        }
    }

    pub(crate) fn display_metrics(cx: &App) -> Screen {
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

    pub(crate) fn hud_enabled(&self) -> bool {
        self.settings.hud != settings::Hud::Off
    }

    pub(crate) fn listening_state(&self) -> Option<UiHud> {
        let l = self.listening.as_ref()?;
        let loading = l.loading_since.map(|since| {
            let frac = since.elapsed().as_secs_f32() / 1.5;
            frac.clamp(0.0, 0.95)
        });
        // Live partial text (H4) only when the setting is on; the model
        // stores `None` otherwise.
        let partial = if self.settings.partials {
            l.partial.clone()
        } else {
            None
        };
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

    pub(crate) fn open_listening_hud(&mut self, cx: &mut App) {
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

    pub(crate) fn hud_origin(&self, cx: &App) -> Option<(f64, f64)> {
        let screen = Self::display_metrics(cx);
        let mouse = if self.settings.hud == settings::Hud::Cursor {
            mouse_position_gpui(screen.height)
        } else {
            None
        };
        hud_origin(self.settings.hud, screen, Self::hud_size(), mouse)
    }

    pub(crate) fn replace_hud(
        &mut self,
        cx: &mut App,
        state: UiHud,
        origin: (f64, f64),
        sticky: bool,
    ) {
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

    pub(crate) fn show_decoding(&mut self, cx: &mut App, dictate: bool, finishing_load: bool) {
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

    pub(crate) fn show_ephemeral(&mut self, cx: &mut App, state: UiHud, tag: &str) {
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
        let dismiss = render_model(&state).dismiss_after_ms;
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

    pub(crate) fn hide_hud(&mut self, cx: &mut App) {
        self.listening = None;
        if let Some(hud) = self.hud.take() {
            close_hud_window(hud.handle, cx);
            self.log("hud hidden");
        }
        self.drain_hud_queue(cx);
    }

    pub(crate) fn drain_hud_queue(&mut self, cx: &mut App) {
        if self.hud.is_some() || !self.hud_enabled() {
            return;
        }
        if let Some(state) = self.hud_queue.pop_front() {
            let dismiss = render_model(&state).dismiss_after_ms;
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

    pub(crate) fn refresh_hud_level(&mut self, cx: &mut App) {
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
            self.refresh_hud_now(cx);
        }
    }

    /// Push the current listening state to the open HUD window (partial
    /// arrivals call this; the level tick calls it when dirty).
    pub(crate) fn refresh_hud_now(&mut self, cx: &mut App) {
        if let (Some(hud), Some(state)) = (&self.hud, self.listening_state()) {
            if hud.sticky {
                let handle = hud.handle;
                if let Err(e) = update_hud_window(handle, state, cx) {
                    self.log(&format!("hud update failed: {e:#}"));
                }
            }
        }
    }
}
