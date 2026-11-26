//! Place math over plain rects: which frame a `Place` covers, which screen
//! holds a window, and Cocoa/AX coordinate conversion. Pure on all OSes.

use jisuno_logic::voice_control::Place;

/// A rect in Cocoa screen coordinates (bottom-left origin, points).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn center(self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn contains(self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// Frame for `place` inside `visible` (an `NSScreen visibleFrame`: menu bar
/// and Dock already excluded). `center` is a 60%-size window in the middle;
/// every other place tiles the visible rect exactly.
pub fn place_frame(visible: Rect, place: Place) -> Rect {
    let Rect { x, y, w, h } = visible;
    match place {
        Place::Full => visible,
        Place::LeftHalf => Rect { x, y, w: w / 2.0, h },
        Place::RightHalf => Rect {
            x: x + w / 2.0,
            y,
            w: w / 2.0,
            h,
        },
        Place::TopHalf => Rect {
            x,
            y: y + h / 2.0,
            w,
            h: h / 2.0,
        },
        Place::BottomHalf => Rect {
            x,
            y,
            w,
            h: h / 2.0,
        },
        Place::LeftTop => Rect {
            x,
            y: y + h / 2.0,
            w: w / 2.0,
            h: h / 2.0,
        },
        Place::LeftBottom => Rect {
            x,
            y,
            w: w / 2.0,
            h: h / 2.0,
        },
        Place::RightTop => Rect {
            x: x + w / 2.0,
            y: y + h / 2.0,
            w: w / 2.0,
            h: h / 2.0,
        },
        Place::RightBottom => Rect {
            x: x + w / 2.0,
            y,
            w: w / 2.0,
            h: h / 2.0,
        },
        Place::Center => Rect {
            x: x + w * 0.2,
            y: y + h * 0.2,
            w: w * 0.6,
            h: h * 0.6,
        },
    }
}

/// Index of the screen containing `(x, y)`, or `None` when no screen does
/// (callers fall back to the main screen).
pub fn screen_at(screens: &[Rect], x: f64, y: f64) -> Option<usize> {
    screens.iter().position(|screen| screen.contains(x, y))
}

/// Cocoa (bottom-left origin) to AX (top-left origin of the primary display).
/// `primary_top` is the primary screen's max Y in Cocoa coordinates; `x` is
/// identical in both spaces.
pub fn cocoa_to_ax(rect: Rect, primary_top: f64) -> Rect {
    Rect {
        x: rect.x,
        y: primary_top - (rect.y + rect.h),
        w: rect.w,
        h: rect.h,
    }
}

/// AX point to Cocoa coordinates (for reading `AXPosition` back).
pub fn ax_point_to_cocoa(x: f64, y: f64, primary_top: f64) -> (f64, f64) {
    (x, primary_top - y)
}

/// Primary screen's max Y in Cocoa coordinates.
pub fn primary_top(main_frame: Rect) -> f64 {
    main_frame.y + main_frame.h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single() -> Rect {
        // 1512x982 with menu bar (24) and Dock (80) excluded.
        Rect {
            x: 0.0,
            y: 80.0,
            w: 1512.0,
            h: 878.0,
        }
    }

    #[test]
    fn all_ten_places_tile_single_screen() {
        let visible = single();
        let cases = [
            (Place::Full, Rect { x: 0.0, y: 80.0, w: 1512.0, h: 878.0 }),
            (Place::LeftHalf, Rect { x: 0.0, y: 80.0, w: 756.0, h: 878.0 }),
            (Place::RightHalf, Rect { x: 756.0, y: 80.0, w: 756.0, h: 878.0 }),
            (Place::TopHalf, Rect { x: 0.0, y: 519.0, w: 1512.0, h: 439.0 }),
            (Place::BottomHalf, Rect { x: 0.0, y: 80.0, w: 1512.0, h: 439.0 }),
            (Place::LeftTop, Rect { x: 0.0, y: 519.0, w: 756.0, h: 439.0 }),
            (Place::LeftBottom, Rect { x: 0.0, y: 80.0, w: 756.0, h: 439.0 }),
            (Place::RightTop, Rect { x: 756.0, y: 519.0, w: 756.0, h: 439.0 }),
            (Place::RightBottom, Rect { x: 756.0, y: 80.0, w: 756.0, h: 439.0 }),
            (
                Place::Center,
                Rect {
                    x: 302.4,
                    y: 255.6,
                    w: 907.1999999999999,
                    h: 526.8,
                },
            ),
        ];
        for (place, want) in cases {
            assert_eq!(place_frame(visible, place), want, "{place:?}");
        }
        // Halves cover the visible rect exactly, with no gap or overlap.
        let left = place_frame(visible, Place::LeftHalf);
        let right = place_frame(visible, Place::RightHalf);
        assert_eq!(left.w + right.w, visible.w);
        assert_eq!(left.h, visible.h);
        let top = place_frame(visible, Place::TopHalf);
        let bottom = place_frame(visible, Place::BottomHalf);
        assert_eq!(top.h + bottom.h, visible.h);
    }

    #[test]
    fn quadrants_cover_visible_rect() {
        let visible = single();
        let quads = [
            place_frame(visible, Place::LeftTop),
            place_frame(visible, Place::LeftBottom),
            place_frame(visible, Place::RightTop),
            place_frame(visible, Place::RightBottom),
        ];
        let area: f64 = quads.iter().map(|rect| rect.w * rect.h).sum();
        assert!((area - visible.w * visible.h).abs() < 1e-6);
    }

    #[test]
    fn screen_pick_single_and_dual() {
        let main = Rect {
            x: 0.0,
            y: 0.0,
            w: 1512.0,
            h: 982.0,
        };
        assert_eq!(screen_at(&[main], 100.0, 100.0), Some(0));
        assert_eq!(screen_at(&[main], 5000.0, 100.0), None);

        // Second display to the right of the main one.
        let right = Rect {
            x: 1512.0,
            y: -58.0,
            w: 1920.0,
            h: 1080.0,
        };
        let screens = [main, right];
        assert_eq!(screen_at(&screens, 100.0, 500.0), Some(0));
        assert_eq!(screen_at(&screens, 2000.0, 500.0), Some(1));
        assert_eq!(screen_at(&screens, -100.0, 500.0), None);
        // A window straddling the boundary picks by its center.
        let straddler = Rect {
            x: 1400.0,
            y: 100.0,
            w: 400.0,
            h: 400.0,
        };
        let (cx, cy) = straddler.center();
        assert_eq!(screen_at(&screens, cx, cy), Some(1));
    }

    #[test]
    fn cocoa_ax_roundtrip() {
        let main = Rect {
            x: 0.0,
            y: 0.0,
            w: 1512.0,
            h: 982.0,
        };
        let top = primary_top(main);
        assert_eq!(top, 982.0);
        // Menu bar sits at AX y 0: the visible rect starts 24pt down.
        let frame = Rect {
            x: 0.0,
            y: 80.0,
            w: 1512.0,
            h: 878.0,
        };
        let ax = cocoa_to_ax(frame, top);
        assert_eq!(ax.y, 24.0);
        // Round trip through the top-left corner.
        let (cx, cy) = ax_point_to_cocoa(ax.x, ax.y, top);
        assert_eq!((cx, cy), (frame.x, frame.y + frame.h));
    }

    #[test]
    fn ax_origin_matches_primary_top_left() {
        // Full-height rect at the main screen origin maps to AX (0, menu).
        let main = Rect {
            x: 0.0,
            y: 0.0,
            w: 1512.0,
            h: 982.0,
        };
        let full = place_frame(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1512.0,
                h: 982.0,
            },
            Place::Full,
        );
        assert_eq!(cocoa_to_ax(full, primary_top(main)), main);
    }
}
