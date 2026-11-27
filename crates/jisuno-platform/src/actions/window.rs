//! `window.focus {window}` and `window.tile {window, place}`.
//!
//! `window` is a [`ResolvedWindow`][super::ResolvedWindow]: a workspace
//! window id resolved to app + title, or a running app name. Focus activates
//! the app and raises the matching `AXWindow` (title match when given).
//! Tile computes the [`Place`][jisuno_logic::voice_control::Place] frame on
//! the screen holding the window from that screen's `visibleFrame` (menu bar
//! and Dock excluded), converts to AX coordinates (top-left origin), and sets
//! `AXPosition` + `AXSize`.

use jisuno_logic::voice_control::Place;

use super::ResolvedWindow;

/// Focus the window: activate the app, raise the matching window.
pub(crate) fn focus_window(resolved: &ResolvedWindow) -> Result<(), String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = resolved;
        Err("jisuno-platform: macOS only".to_string())
    }
    #[cfg(target_os = "macos")]
    {
        macos::focus_window(resolved)
    }
}

/// Move and resize the window to `place` on the screen holding it.
pub(crate) fn tile_window(resolved: &ResolvedWindow, place: Place) -> Result<(), String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (resolved, place);
        Err("jisuno-platform: macOS only".to_string())
    }
    #[cfg(target_os = "macos")]
    {
        macos::tile_window(resolved, place)
    }
}

/// Snapshot of one display for [`super::geom`] math: full and visible frames
/// in Cocoa coordinates (bottom-left origin).
#[cfg(target_os = "macos")]
pub(crate) struct ScreenInfo {
    pub(crate) frame: super::geom::Rect,
    pub(crate) visible: super::geom::Rect,
    /// Primary screen's max Y: the AX coordinate flip line.
    pub(crate) primary_top: f64,
}

#[cfg(target_os = "macos")]
pub(crate) mod macos {
    use super::super::ax::macos as axm;
    use super::super::geom;
    use super::*;

    fn no_window(resolved: &ResolvedWindow) -> String {
        format!("no window \"{}\"", resolved.label)
    }

    /// Launch the app when it is not running (workspace defs name apps that
    /// may be closed), then return its pid.
    pub(crate) fn ensure_running(resolved: &ResolvedWindow) -> Result<i32, String> {
        if let Some(pid) = axm::pid_for_app(&resolved.app) {
            return Ok(pid);
        }
        super::super::app::open_app(&resolved.app, None)?;
        // The app was just launched: poll briefly for its first window.
        // (Longer waits are the `wait` step's job.)
        for _ in 0..20 {
            if let Some(pid) = axm::pid_for_app(&resolved.app) {
                return Ok(pid);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        Err(no_window(resolved))
    }

    /// Activate the app (no-op when it is already frontmost).
    pub(crate) fn activate(pid: i32) {
        use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};

        if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
            let options = NSApplicationActivationOptions::ActivateAllWindows;
            app.activateWithOptions(options);
        }
    }

    /// Snapshot every display. Index 0 is the main screen. Empty off the
    /// main thread or with no displays.
    pub(crate) fn screens() -> Vec<ScreenInfo> {
        use objc2::MainThreadMarker;
        use objc2_app_kit::NSScreen;

        let Some(mtm) = MainThreadMarker::new() else {
            return Vec::new();
        };

        let main_frame = NSScreen::mainScreen(mtm).map(|screen| {
            let frame = screen.frame();
            geom::Rect {
                x: frame.origin.x,
                y: frame.origin.y,
                w: frame.size.width,
                h: frame.size.height,
            }
        });
        let primary_top = main_frame.map(geom::primary_top).unwrap_or(0.0);
        NSScreen::screens(mtm)
            .iter()
            .map(|screen| {
                let frame = screen.frame();
                let visible = screen.visibleFrame();
                ScreenInfo {
                    frame: geom::Rect {
                        x: frame.origin.x,
                        y: frame.origin.y,
                        w: frame.size.width,
                        h: frame.size.height,
                    },
                    visible: geom::Rect {
                        x: visible.origin.x,
                        y: visible.origin.y,
                        w: visible.size.width,
                        h: visible.size.height,
                    },
                    primary_top,
                }
            })
            .collect()
    }

    /// AX-space frame for `place` on the screen holding the window element:
    /// read `AXPosition`, pick the containing screen (main screen fallback),
    /// run [`geom::place_frame`] on its visible rect, flip to AX coordinates.
    pub(crate) fn target_frame_ax(
        element: *const std::ffi::c_void,
        place: Place,
        screens: &[ScreenInfo],
    ) -> Option<geom::Rect> {
        let screen = match axm::read_point(element).ok().flatten() {
            Some([x, y]) => {
                // `AXPosition` is top-left origin: flip to Cocoa for picking.
                let (cx, _) = geom::ax_point_to_cocoa(x, y, screens.first()?.primary_top);
                let cy = screens.first()?.primary_top - y;
                let index = geom::screen_at(
                    &screens
                        .iter()
                        .map(|screen| screen.frame)
                        .collect::<Vec<_>>(),
                    cx,
                    cy,
                )
                .unwrap_or(0);
                screens.get(index)?
            }
            None => screens.first()?,
        };
        let frame = geom::place_frame(screen.visible, place);
        Some(geom::cocoa_to_ax(frame, screen.primary_top))
    }

    pub(super) fn focus_window(resolved: &ResolvedWindow) -> Result<(), String> {
        let pid = ensure_running(resolved)?;
        activate(pid);
        // The window may still be opening: poll briefly, then raise it.
        let mut found = None;
        for _ in 0..20 {
            match axm::find_window_element(resolved) {
                Ok((windows, element)) => {
                    found = Some((windows, element));
                    break;
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
            }
        }
        let Some((windows, element)) = found else {
            return Err(no_window(resolved));
        };
        // Keep the list alive across the raise (see `ax::press`).
        let _windows = windows;
        if axm::perform(element, "AXRaise") != axm::AX_OK {
            return Err(no_window(resolved));
        }
        Ok(())
    }

    pub(super) fn tile_window(resolved: &ResolvedWindow, place: Place) -> Result<(), String> {
        if crate::permissions::accessibility() != crate::permissions::PermissionState::Granted {
            return Err("tile failed: no permission".to_string());
        }
        let pid = ensure_running(resolved)?;
        activate(pid);
        let (windows, element) =
            axm::find_window_element(resolved).map_err(|_| no_window(resolved))?;
        // Keep the list alive across the reads and writes below.
        let _windows = windows;
        let screens = screens();
        if screens.is_empty() {
            return Err("tile failed: no permission".to_string());
        }
        let Some(frame) = target_frame_ax(element, place, &screens) else {
            return Err(no_window(resolved));
        };
        // The trust check above covers the permission case; any set failure
        // surfaces under the same spec reason.
        if axm::set_frame_ax(element, frame.x, frame.y, frame.w, frame.h) != axm::AX_OK {
            return Err("tile failed: no permission".to_string());
        }
        Ok(())
    }
}
