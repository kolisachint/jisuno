//! Hosting the HUD island in its own window (macOS only).
//!
//! The HUD window is a GPUI [`WindowKind::PopUp`]: on macOS that maps to a
//! non-activating `NSPanel` at the pop-up level (above normal windows, never
//! key), which is exactly the `app-ui.md` §3 island: borderless (transparent
//! titlebar, full-size content view, transparent background), always on top,
//! never stealing focus. It joins all Spaces and follows the active one
//! (`CanJoinAllSpaces | FullScreenAuxiliary` in gpui-pre-macos).
//!
//! The app opens one window per HUD state via [`open_hud_window`] and closes
//! it via [`close_hud_window`]; the window is fixed-size (see
//! [`HUD_WINDOW_SIZE`]) with transparent margins around the island, since the
//! island height varies with partial text and notes.

use gpui::{
    prelude::*, px, size, AnyWindowHandle, App, Bounds, Point, WindowBounds, WindowHandle,
    WindowKind, WindowOptions,
};

use crate::hud_view::HudView;
use crate::{hud_model::HudState, tokens::HUD_MAX_W};

/// Fixed HUD window size in points: the island max width (§1.3) and room
/// for the mode row plus two detail lines. The island draws top-left;
/// transparent margins show the desktop through.
pub const HUD_WINDOW_SIZE: (f32, f32) = (HUD_MAX_W, 190.0);

/// Open the HUD window at `origin` (GPUI global points, e.g. from
/// `jisuno_platform::hud_placement::hud_origin`) showing `state`.
pub fn open_hud_window(
    cx: &mut App,
    state: HudState,
    origin: (f64, f64),
) -> anyhow::Result<WindowHandle<HudView>> {
    let (w, h) = HUD_WINDOW_SIZE;
    let bounds = Bounds {
        origin: Point {
            x: px(origin.0 as f32),
            y: px(origin.1 as f32),
        },
        size: size(px(w), px(h)),
    };
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            // No titlebar at all: with `Some`, gpui-pre-macos adds the
            // Closable mask (traffic lights) and `None` position restores
            // the default buttons. Bare Titled|FullSizeContentView draws
            // nothing, which is the borderless HUD island.
            titlebar: None,
            focus: false,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            window_background: gpui::WindowBackgroundAppearance::Transparent,
            ..Default::default()
        },
        |_, cx| cx.new(|_| HudView::new(state)),
    )
}

/// Update the showing HUD window's state, if it is still open.
pub fn update_hud_window(
    handle: AnyWindowHandle,
    state: HudState,
    cx: &mut App,
) -> anyhow::Result<()> {
    let handle: WindowHandle<HudView> = handle
        .downcast()
        .ok_or_else(|| anyhow::anyhow!("HUD window changed type"))?;
    handle.update(cx, |view, _, cx| {
        view.set_state(state, cx);
    })?;
    Ok(())
}

/// Close the HUD window, if it is still open.
pub fn close_hud_window(handle: AnyWindowHandle, cx: &mut App) {
    if let Some(typed) = handle.downcast::<HudView>() {
        let _ = typed.update(cx, |_, window, _| {
            window.remove_window();
        });
    }
}
