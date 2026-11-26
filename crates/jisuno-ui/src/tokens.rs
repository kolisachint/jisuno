//! Theme tokens: inkuno Quiet light/dark palettes, Jisuno's dictate/command
//! accents, and the type, spacing, radii and motion scales from `app-ui.md` §1.
//!
//! Pure: no GPUI dependency, so Linux CI builds and tests this module.

use anyhow::{anyhow, Result};

/// One Quiet palette: every hex exactly as `app-ui.md` §1.1 lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub bg: u32,
    pub panel: u32,
    pub ground: u32,
    pub code_bg: u32,
    pub popover: u32,
    pub border: u32,
    pub text: u32,
    pub soft: u32,
    pub selected: u32,
    pub chip: u32,
    pub chip_line: u32,
    pub caution: u32,
    pub ok: u32,
    pub warning: u32,
}

/// Quiet light (`theme.json` `quiet-light`).
pub const QUIET_LIGHT: Palette = Palette {
    bg: 0xffffff,
    panel: 0xffffff,
    ground: 0xeff0f1,
    code_bg: 0xfbfbfb,
    popover: 0xfafafa,
    border: 0xeaebeb,
    text: 0x1f2225,
    soft: 0x686a6c,
    selected: 0xd8e7fd,
    chip: 0xf6f6f7,
    chip_line: 0xe2e3e3,
    caution: 0xc93a3a,
    ok: 0x2a7b4d,
    warning: 0x9e5f09,
};

/// Quiet dark (`theme.json` `quiet-dark`).
pub const QUIET_DARK: Palette = Palette {
    bg: 0x1c1c1e,
    panel: 0x2c2c2e,
    ground: 0x141517,
    code_bg: 0x232325,
    popover: 0x2a2a2c,
    border: 0x38393b,
    text: 0xe6e6e6,
    soft: 0x959596,
    selected: 0x243654,
    chip: 0x262628,
    chip_line: 0x3a3b3e,
    caution: 0xff7b72,
    ok: 0x58c48a,
    warning: 0xe0a23c,
};

/// The selected sidebar pill is the text at 7.2 % (Quiet rule, §1.1).
pub const PILL_ALPHA: f32 = 0.072;

/// Jisuno's two accent fills, the same in both palettes (§1.1).
pub const DICTATE_FILL: u32 = 0x3a85f7;
pub const COMMAND_FILL: u32 = 0xe39b17;

/// Dictate text/glyph colours: `legible` at 4.5:1 from the fill (§1.1).
pub const DICTATE_TEXT_LIGHT: u32 = 0x095ad7;
pub const DICTATE_TEXT_DARK: u32 = 0x7faffa;
/// Command text/glyph colours: `legible` at 4.5:1 from the fill (§1.1).
pub const COMMAND_TEXT_LIGHT: u32 = 0x865c0e;
pub const COMMAND_TEXT_DARK: u32 = 0xe8a01c;

/// Primary button label: white on light, near-black on dark (§1.1).
pub const BUTTON_LABEL_LIGHT: u32 = 0xffffff;
pub const BUTTON_LABEL_DARK: u32 = 0x1c1c1e;

/// The HUD island is always dark, whatever the palette (§1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HudTokens {
    pub bg: u32,
    pub text: u32,
    pub soft: u32,
    pub chip: u32,
    pub dictate: u32,
    pub command: u32,
    pub ok: u32,
    pub err: u32,
}

pub const HUD_TOKENS: HudTokens = HudTokens {
    bg: 0x0b0b0c,
    text: 0xf2f2f3,
    soft: 0x8b8d91,
    chip: 0x26272b,
    dictate: 0x7faffa,
    command: 0xe8a01c,
    ok: 0x58c48a,
    err: 0xff7b72,
};

/// Type roles from §1.2: size/line in px, weight, and whether the role is
/// secondary (`soft`) or mono. macOS system fonts (SF Pro / SF Mono); nothing
/// is bundled, so no font files or family names ship here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TypeRole {
    pub size: f32,
    pub line: f32,
    pub weight: Weight,
    pub soft: bool,
    pub mono: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    Regular,
    Medium,
    Semibold,
}

pub const DISPLAY: TypeRole = TypeRole {
    size: 20.0,
    line: 26.0,
    weight: Weight::Semibold,
    soft: false,
    mono: false,
};
pub const TITLE: TypeRole = TypeRole {
    size: 15.0,
    line: 22.0,
    weight: Weight::Semibold,
    soft: false,
    mono: false,
};
pub const BODY: TypeRole = TypeRole {
    size: 13.0,
    line: 18.0,
    weight: Weight::Regular,
    soft: false,
    mono: false,
};
pub const LABEL: TypeRole = TypeRole {
    size: 13.0,
    line: 18.0,
    weight: Weight::Medium,
    soft: false,
    mono: false,
};
pub const CAPTION: TypeRole = TypeRole {
    size: 12.0,
    line: 16.0,
    weight: Weight::Regular,
    soft: true,
    mono: false,
};
pub const MICRO: TypeRole = TypeRole {
    size: 11.0,
    line: 14.0,
    weight: Weight::Medium,
    soft: false,
    mono: false,
};
pub const MONO: TypeRole = TypeRole {
    size: 12.0,
    line: 18.0,
    weight: Weight::Regular,
    soft: false,
    mono: true,
};
pub const KBD: TypeRole = TypeRole {
    size: 11.0,
    line: 14.0,
    weight: Weight::Regular,
    soft: false,
    mono: true,
};

/// Spacing (§1.3): 4 px grid, controls at least 26 px high.
pub const GRID: f32 = 4.0;
pub const CONTROL_MIN_H: f32 = 26.0;
pub const POPOVER_ROW_H: f32 = 26.0;
pub const SETTINGS_ROW_PAD: f32 = 8.0;

/// Window sizes (§4 popover, §5 Settings, §6 Welcome).
pub const POPOVER_W: f32 = 310.0;
pub const POPOVER_PAD: f32 = 6.0;
pub const SETTINGS_W: f32 = 800.0;
pub const SETTINGS_H: f32 = 520.0;
pub const SETTINGS_SIDEBAR_W: f32 = 168.0;
pub const SETTINGS_PAD: f32 = 20.0;
pub const WELCOME_W: f32 = 540.0;
pub const WELCOME_H: f32 = 580.0;

/// HUD island sizes (§1.3, prototype `v3.html`).
pub const HUD_MIN_W: f32 = 190.0;
pub const HUD_MAX_W: f32 = 560.0;
pub const HUD_PAD_X: f32 = 16.0;
pub const HUD_PAD_TOP_NOTCH: f32 = 34.0;
pub const HUD_PAD_BOTTOM_NOTCH: f32 = 10.0;
pub const HUD_PAD_CURSOR: f32 = 10.0;
pub const HUD_CURSOR_GAP: f32 = 24.0;
pub const HUD_BAR_H: f32 = 3.0;
pub const HUD_SPINNER: f32 = 12.0;

/// Radii (§1.3).
pub const RADIUS_KBD: f32 = 4.0;
pub const RADIUS_CONTROL: f32 = 6.0;
pub const RADIUS_CARD: f32 = 8.0;
pub const RADIUS_WINDOW: f32 = 12.0;
pub const RADIUS_HUD_NOTCH_BOTTOM: f32 = 18.0;
pub const RADIUS_HUD_CURSOR: f32 = 14.0;

/// Motion durations in ms (§1.4).
pub const MOTION_FAST_MS: u64 = 90;
pub const MOTION_BASE_MS: u64 = 180;
pub const MOTION_SLOW_MS: u64 = 350;
pub const MOTION_PULSE_MS: u64 = 800;
pub const MOTION_SPIN_MS: u64 = 600;

/// Parse `#rrggbb` (the `#` may be omitted); `None` when it is not one.
pub fn parse_hex(text: &str) -> Option<u32> {
    let text = text.strip_prefix('#').unwrap_or(text);
    (text.len() == 6 && text.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| u32::from_str_radix(text, 16).ok())
        .flatten()
}

fn channel(v: u32, shift: u32) -> f32 {
    ((v >> shift) & 0xff) as f32 / 255.0
}

fn luminance(hex: u32) -> f32 {
    let linear = |x: f32| {
        if x <= 0.03928 {
            x / 12.92
        } else {
            ((x + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(channel(hex, 16))
        + 0.7152 * linear(channel(hex, 8))
        + 0.0722 * linear(channel(hex, 0))
}

/// WCAG contrast ratio of two hex colours: at least 4.5 for body text.
pub fn contrast(a: u32, b: u32) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn rgb_to_hsl(hex: u32) -> (f32, f32, f32) {
    let (r, g, b) = (channel(hex, 16), channel(hex, 8), channel(hex, 0));
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    if max == min {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    (h, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> u32 {
    if s == 0.0 {
        let c = (l * 255.0).round() as u32;
        return (c << 16) | (c << 8) | c;
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let f = |t: f32| {
        let t = if t < 0.0 {
            t + 1.0
        } else if t > 1.0 {
            t - 1.0
        } else {
            t
        };
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    let c = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u32;
    (c(f(h + 1.0 / 3.0)) << 16) | (c(f(h)) << 8) | c(f(h - 1.0 / 3.0))
}

/// `color`, darkened on a light ground or lightened on a dark one, just
/// enough to read on `bg` at `ratio`: inkuno's `reach` rule (§1.1).
pub fn reach(color: u32, bg: u32, ratio: f32) -> u32 {
    let step = if luminance(bg) > 0.18 { -0.02 } else { 0.02 };
    let (h, s, mut l) = rgb_to_hsl(color);
    while contrast(hsl_to_rgb(h, s, l), bg) < ratio && (0.0..=1.0).contains(&(l + step)) {
        l += step;
    }
    hsl_to_rgb(h, s, l)
}

/// The legible text variant of an accent fill on `bg` (4.5:1, §1.1).
pub fn legible(color: u32, bg: u32) -> u32 {
    reach(color, bg, 4.5)
}

/// The typed `theme.json` document (`schemas/theme.schema.json`). Every key is
/// optional in the file; a missing key takes its default. Kept local:
/// `jisuno-logic` has no theme module yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeJson {
    pub name: String,
    pub appearance: String,
    pub accent_dictate: String,
    pub accent_command: String,
    pub hud_style: String,
    pub show_latency: bool,
}

impl Default for ThemeJson {
    fn default() -> Self {
        Self {
            name: "quiet-light".into(),
            appearance: "system".into(),
            accent_dictate: "#3a85f7".into(),
            accent_command: "#e39b17".into(),
            hud_style: "island".into(),
            show_latency: false,
        }
    }
}

impl ThemeJson {
    /// Read the defaults file (`crates/jisuno-logic/defaults/theme.json`).
    pub fn defaults() -> Self {
        Self::from_value(&serde_json::json!({})).expect("empty theme.json takes all defaults")
    }

    /// Parse a `theme.json` value: unknown keys are errors, bad values are errors.
    pub fn from_value(value: &serde_json::Value) -> Result<Self> {
        let obj = value
            .as_object()
            .ok_or_else(|| anyhow!("theme: expected an object"))?;
        for key in obj.keys() {
            if !matches!(
                key.as_str(),
                "$schema" | "name" | "appearance" | "accent_dictate" | "accent_command" | "hud"
            ) {
                return Err(anyhow!("theme: unknown key {key:?}"));
            }
        }
        let mut out = Self::default();
        let get_str = |key: &str| -> Result<Option<String>> {
            match obj.get(key) {
                None => Ok(None),
                Some(v) => Ok(Some(
                    v.as_str()
                        .ok_or_else(|| anyhow!("theme: {key:?} must be a string"))?
                        .to_string(),
                )),
            }
        };
        if let Some(name) = get_str("name")? {
            if !matches!(name.as_str(), "quiet-light" | "quiet-dark") {
                return Err(anyhow!("theme: name must be quiet-light or quiet-dark"));
            }
            out.name = name;
        }
        if let Some(appearance) = get_str("appearance")? {
            if !matches!(appearance.as_str(), "system" | "fixed") {
                return Err(anyhow!("theme: appearance must be system or fixed"));
            }
            out.appearance = appearance;
        }
        for key in ["accent_dictate", "accent_command"] {
            if let Some(accent) = get_str(key)? {
                if parse_hex(&accent).is_none() || !accent.starts_with('#') {
                    return Err(anyhow!("theme: {key:?} must be #rrggbb (lowercase)"));
                }
                if accent != accent.to_lowercase() {
                    return Err(anyhow!("theme: {key:?} must be #rrggbb (lowercase)"));
                }
                if key == "accent_dictate" {
                    out.accent_dictate = accent;
                } else {
                    out.accent_command = accent;
                }
            }
        }
        if let Some(hud) = obj.get("hud") {
            let hud = hud
                .as_object()
                .ok_or_else(|| anyhow!("theme: \"hud\" must be an object"))?;
            for key in hud.keys() {
                if !matches!(key.as_str(), "style" | "show_latency") {
                    return Err(anyhow!("theme: unknown hud key {key:?}"));
                }
            }
            if let Some(style) = hud.get("style") {
                let style = style
                    .as_str()
                    .ok_or_else(|| anyhow!("theme: hud.style must be a string"))?;
                if style != "island" {
                    return Err(anyhow!("theme: hud.style is island in v1"));
                }
                out.hud_style = style.into();
            }
            if let Some(show) = hud.get("show_latency") {
                out.show_latency = show
                    .as_bool()
                    .ok_or_else(|| anyhow!("theme: hud.show_latency must be a boolean"))?;
            }
        }
        Ok(out)
    }
}

/// The resolved theme: a Quiet palette plus the accent fills and their
/// legible text variants. A custom `accent_dictate`/`accent_command` keeps its
/// fill; the text variant is derived by the legibility rule (§1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub palette: Palette,
    pub dark: bool,
    pub dictate_fill: u32,
    pub dictate_text: u32,
    pub command_fill: u32,
    pub command_text: u32,
    pub button_label: u32,
    pub show_latency: bool,
}

impl Theme {
    pub fn quiet_light() -> Self {
        Self {
            palette: QUIET_LIGHT,
            dark: false,
            dictate_fill: DICTATE_FILL,
            dictate_text: DICTATE_TEXT_LIGHT,
            command_fill: COMMAND_FILL,
            command_text: COMMAND_TEXT_LIGHT,
            button_label: BUTTON_LABEL_LIGHT,
            show_latency: false,
        }
    }

    pub fn quiet_dark() -> Self {
        Self {
            palette: QUIET_DARK,
            dark: true,
            dictate_fill: DICTATE_FILL,
            dictate_text: DICTATE_TEXT_DARK,
            command_fill: COMMAND_FILL,
            command_text: COMMAND_TEXT_DARK,
            button_label: BUTTON_LABEL_DARK,
            show_latency: false,
        }
    }

    /// Resolve `theme.json`: the fixed palette by `name`, the accent fills as
    /// written, the text variants derived for legibility (§1.1, §12:
    /// `appearance: fixed` uses `name`; `system` needs the macOS appearance,
    /// which the macOS shell passes as `system_dark`).
    pub fn from_theme_json(value: &serde_json::Value, system_dark: bool) -> Result<Self> {
        let doc = ThemeJson::from_value(value)?;
        let dark = match doc.appearance.as_str() {
            "fixed" => doc.name == "quiet-dark",
            _ => system_dark,
        };
        let mut theme = if dark {
            Self::quiet_dark()
        } else {
            Self::quiet_light()
        };
        // A missing key takes its default, including the hand-worked text
        // variants; a custom fill derives its text for legibility (§1.1).
        let defaults = ThemeJson::default();
        let bg = theme.palette.bg;
        if doc.accent_dictate != defaults.accent_dictate {
            if let Some(fill) = parse_hex(&doc.accent_dictate) {
                theme.dictate_fill = fill;
                theme.dictate_text = legible(fill, bg);
            }
        }
        if doc.accent_command != defaults.accent_command {
            if let Some(fill) = parse_hex(&doc.accent_command) {
                theme.command_fill = fill;
                // The command fill is never used as text or glyph on light (§1.1);
                // the text variant still derives so a custom accent keeps readable text.
                theme.command_text = legible(fill, bg);
            }
        }
        theme.show_latency = doc.show_latency;
        Ok(theme)
    }
}

/// Countdown meta from the ms left: `0:15 left` down to `0:00 left` (§3.3 H5).
/// Rounds up, as the prototype does, so the first tick still reads `0:15 left`.
pub fn format_countdown(ms_left: u64) -> String {
    let secs = ms_left.div_ceil(1000);
    format!("{}:{:02} left", secs / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_palettes_match_the_spec_table() {
        assert_eq!(QUIET_LIGHT.bg, 0xffffff);
        assert_eq!(QUIET_LIGHT.ground, 0xeff0f1);
        assert_eq!(QUIET_LIGHT.text, 0x1f2225);
        assert_eq!(QUIET_LIGHT.soft, 0x686a6c);
        assert_eq!(QUIET_DARK.bg, 0x1c1c1e);
        assert_eq!(QUIET_DARK.ground, 0x141517);
        assert_eq!(QUIET_DARK.text, 0xe6e6e6);
        assert_eq!(HUD_TOKENS.bg, 0x0b0b0c);
        assert_eq!(HUD_TOKENS.dictate, 0x7faffa);
        assert_eq!(HUD_TOKENS.command, 0xe8a01c);
        assert_eq!((DICTATE_FILL, COMMAND_FILL), (0x3a85f7, 0xe39b17));
    }

    #[test]
    fn spec_accents_stay_legible() {
        for (text, bg) in [
            (DICTATE_TEXT_LIGHT, QUIET_LIGHT.bg),
            (COMMAND_TEXT_LIGHT, QUIET_LIGHT.bg),
            (DICTATE_TEXT_DARK, QUIET_DARK.bg),
            (COMMAND_TEXT_DARK, QUIET_DARK.bg),
        ] {
            assert!(contrast(text, bg) >= 4.5, "{text:06x} on {bg:06x}");
        }
    }

    #[test]
    fn derived_text_matches_the_spec_accents() {
        // The derivation recovers the spec's hand-worked text variants closely:
        // same hue family and legible; the table values stay the stored tokens.
        for ((fill, bg, spec), name) in [
            (
                (DICTATE_FILL, QUIET_LIGHT.bg, DICTATE_TEXT_LIGHT),
                "dictate light",
            ),
            (
                (COMMAND_FILL, QUIET_LIGHT.bg, COMMAND_TEXT_LIGHT),
                "command light",
            ),
            (
                (DICTATE_FILL, QUIET_DARK.bg, DICTATE_TEXT_DARK),
                "dictate dark",
            ),
            (
                (COMMAND_FILL, QUIET_DARK.bg, COMMAND_TEXT_DARK),
                "command dark",
            ),
        ] {
            let derived = legible(fill, bg);
            assert!(contrast(derived, bg) >= 4.5, "{name}");
            let (dh, ds, _) = rgb_to_hsl(derived);
            let (sh, ss, _) = rgb_to_hsl(spec);
            let hue_gap = (dh - sh).abs().min(1.0 - (dh - sh).abs());
            assert!(
                hue_gap < 0.06 && (ds - ss).abs() < 0.35,
                "{name}: {derived:06x}"
            );
        }
    }

    #[test]
    fn theme_json_defaults_match_quiet_light() {
        let theme = Theme::from_theme_json(&serde_json::json!({}), false).unwrap();
        assert_eq!(theme, Theme::quiet_light());
    }

    #[test]
    fn theme_json_fixed_dark_and_system_follow() {
        let dark = Theme::from_theme_json(
            &serde_json::json!({"name": "quiet-dark", "appearance": "fixed"}),
            false,
        )
        .unwrap();
        assert!(dark.dark && dark.palette == QUIET_DARK);
        let system = Theme::from_theme_json(&serde_json::json!({}), true).unwrap();
        assert!(system.dark && system.palette == QUIET_DARK);
    }

    #[test]
    fn theme_json_applies_accent_overrides_legibly() {
        let theme = Theme::from_theme_json(
            &serde_json::json!({"accent_dictate": "#00aa00", "accent_command": "#aa0000"}),
            false,
        )
        .unwrap();
        assert_eq!(theme.dictate_fill, 0x00aa00);
        assert_eq!(theme.command_fill, 0xaa0000);
        assert!(contrast(theme.dictate_text, theme.palette.bg) >= 4.5);
        assert!(contrast(theme.command_text, theme.palette.bg) >= 4.5);
    }

    #[test]
    fn theme_json_rejects_bad_values() {
        for value in [
            serde_json::json!({"nope": 1}),
            serde_json::json!({"name": "solarized"}),
            serde_json::json!({"accent_dictate": "red"}),
            serde_json::json!({"accent_dictate": "#3A85F7"}),
            serde_json::json!({"hud": {"style": "cursor"}}),
            serde_json::json!({"hud": {"show_latency": "yes"}}),
        ] {
            assert!(ThemeJson::from_value(&value).is_err(), "{value}");
        }
    }

    #[test]
    fn countdown_formats_like_the_prototype() {
        assert_eq!(format_countdown(15_000), "0:15 left");
        assert_eq!(format_countdown(14_001), "0:15 left");
        assert_eq!(format_countdown(14_000), "0:14 left");
        assert_eq!(format_countdown(1), "0:01 left");
        assert_eq!(format_countdown(0), "0:00 left");
        assert_eq!(format_countdown(60_000), "1:00 left");
    }

    #[test]
    fn motion_and_geometry_match_the_spec() {
        assert_eq!(
            (
                MOTION_FAST_MS,
                MOTION_BASE_MS,
                MOTION_SLOW_MS,
                MOTION_PULSE_MS,
                MOTION_SPIN_MS
            ),
            (90, 180, 350, 800, 600)
        );
        assert_eq!((RADIUS_HUD_NOTCH_BOTTOM, RADIUS_HUD_CURSOR), (18.0, 14.0));
        assert_eq!((HUD_MIN_W, HUD_MAX_W), (190.0, 560.0));
        assert_eq!(CONTROL_MIN_H, 26.0);
    }
}
