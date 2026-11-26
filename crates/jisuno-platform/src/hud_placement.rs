//! HUD window placement: where the island goes for each `hud` setting.
//!
//! `app-ui.md` §3.1: `notch` hangs from the bottom edge of the notch (top
//! center, under the menu bar; on a Mac without a notch, under the menu bar,
//! centred); `cursor` sits 24 px below the mouse pointer, centred;
//! `off` draws nothing. All coordinates here are GPUI global points:
//! origin at the top-left of the primary display, y grows downward — the
//! same space as `PlatformDisplay::bounds()`, so the app can pass the result
//! straight into `WindowBounds::Windowed`.
//!
//! Pure: unit-tested on all OSes. The mouse query is macOS-only.

/// A display's size in points, from `PlatformDisplay::bounds().size`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Screen {
    /// Full display size, including the menu bar area.
    pub width: f64,
    /// Full display size, including the menu bar area.
    pub height: f64,
    /// Top of the usable area (below the menu bar), in the same space as
    /// `PlatformDisplay::visible_bounds().origin.y`.
    pub usable_top: f64,
}

/// The HUD window size in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HudSize {
    pub width: f64,
    pub height: f64,
}

/// Gap between the mouse pointer and the top of the cursor HUD (§3.1).
pub const CURSOR_GAP: f64 = 24.0;

/// Top-left origin for the HUD window, or `None` for `hud: off` (and when
/// `hud: cursor` but the pointer position is unknown).
pub fn hud_origin(
    hud: jisuno_logic::settings::Hud,
    screen: Screen,
    hud_size: HudSize,
    mouse: Option<(f64, f64)>,
) -> Option<(f64, f64)> {
    use jisuno_logic::settings::Hud;
    match hud {
        Hud::Off => None,
        Hud::Notch => Some((
            ((screen.width - hud_size.width) / 2.0).max(0.0),
            screen.usable_top,
        )),
        Hud::Cursor => {
            let (mx, my) = mouse?;
            let x = (mx - hud_size.width / 2.0).clamp(0.0, (screen.width - hud_size.width).max(0.0));
            let y = (my + CURSOR_GAP).clamp(0.0, (screen.height - hud_size.height).max(0.0));
            Some((x, y))
        }
    }
}

/// The mouse pointer in GPUI global points (`None` when it cannot be read).
/// Converts the AppKit bottom-left global position into the top-left space
/// using the primary display height. Call from the main thread.
pub fn mouse_position_gpui(primary_height: f64) -> Option<(f64, f64)> {
    #[cfg(target_os = "macos")]
    {
        let point = objc2_app_kit::NSEvent::mouseLocation();
        if point.x.is_finite() && point.y.is_finite() {
            return Some((point.x, primary_height - point.y));
        }
        None
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = primary_height;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jisuno_logic::settings::Hud;

    fn screen() -> Screen {
        Screen {
            width: 1512.0,
            height: 982.0,
            usable_top: 24.0,
        }
    }

    fn size() -> HudSize {
        HudSize {
            width: 560.0,
            height: 170.0,
        }
    }

    #[test]
    fn notch_is_top_center_under_the_menu_bar() {
        assert_eq!(
            hud_origin(Hud::Notch, screen(), size(), None),
            Some(((1512.0 - 560.0) / 2.0, 24.0))
        );
    }

    #[test]
    fn cursor_is_24px_below_the_pointer_centred() {
        assert_eq!(
            hud_origin(Hud::Cursor, screen(), size(), Some((800.0, 400.0))),
            Some((800.0 - 280.0, 424.0))
        );
    }

    #[test]
    fn cursor_clamps_to_the_display() {
        // Bottom-right corner: clamped inside.
        assert_eq!(
            hud_origin(Hud::Cursor, screen(), size(), Some((1500.0, 970.0))),
            Some((1512.0 - 560.0, 982.0 - 170.0))
        );
        // Top-left corner: clamped at zero.
        assert_eq!(
            hud_origin(Hud::Cursor, screen(), size(), Some((10.0, 0.0))),
            Some((0.0, 24.0))
        );
    }

    #[test]
    fn off_and_unknown_mouse_draw_nothing() {
        assert_eq!(hud_origin(Hud::Off, screen(), size(), Some((1.0, 1.0))), None);
        assert_eq!(hud_origin(Hud::Cursor, screen(), size(), None), None);
    }

    #[test]
    fn narrow_display_never_goes_negative() {
        let small = Screen {
            width: 400.0,
            height: 300.0,
            usable_top: 24.0,
        };
        assert_eq!(
            hud_origin(Hud::Notch, small, size(), None),
            Some((0.0, 24.0))
        );
    }
}
