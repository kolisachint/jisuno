//! The HUD island view (macOS only): renders a [`HudState`] in the island
//! style at the sizes in `app-ui.md` §§1/3 and the prototype `v3.html`.
//!
//! The island is always dark, whatever the palette (§1.1). Level bars redraw
//! with the state (90 ms in the app); the spinner is a static ring and the
//! island does not animate on its own, so there is nothing to disable for
//! Reduce Motion — gpui-pre 0.3 also exposes no reduced-motion query. The
//! view sets no font files: text draws in the macOS system fonts (SF Pro for
//! UI, SF Mono for the meta/countdown), which CoreText resolves by name.

use gpui::{
    div, hsla, point, prelude::*, px, rgb, BoxShadow, Context, DefiniteLength, Div, FontWeight,
};

use crate::hud_model::{
    detail_lines, render_model, tail_text, HudAccent, HudIcon, HudState, PARTIAL_TAIL_CHARS,
};
use crate::tokens::{
    BODY, CAPTION, HUD_BAR_H, HUD_MAX_W, HUD_MIN_W, HUD_PAD_BOTTOM_NOTCH, HUD_PAD_TOP_NOTCH,
    HUD_PAD_X, HUD_SPINNER, HUD_TOKENS, MICRO,
};

/// A HUD island showing one state. The app replaces the state (and the view)
/// on every HUD change; one state at a time (§3.2).
pub struct HudView {
    state: HudState,
}

impl HudView {
    pub fn new(state: HudState) -> Self {
        Self { state }
    }

    /// Replace the shown state (the app calls this, or rebuilds the view).
    pub fn set_state(&mut self, state: HudState, cx: &mut Context<Self>) {
        self.state = state;
        cx.notify();
    }
}

impl Render for HudView {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut Context<Self>) -> impl IntoElement {
        hud_island(&self.state)
    }
}

fn accent_text(accent: HudAccent) -> u32 {
    match accent {
        HudAccent::Dictate => HUD_TOKENS.dictate,
        HudAccent::Command => HUD_TOKENS.command,
        HudAccent::Ok => HUD_TOKENS.ok,
        HudAccent::Error => HUD_TOKENS.err,
        HudAccent::Neutral => HUD_TOKENS.text,
    }
}

/// The island: dark chrome, bottom corners 18 px (notch), spec padding and
/// the floating shadow (§1.3, prototype `v3.html`).
fn island_base() -> Div {
    div()
        .min_w(px(HUD_MIN_W))
        .max_w(px(HUD_MAX_W))
        .bg(rgb(HUD_TOKENS.bg))
        .rounded_b(px(18.0))
        .px(px(HUD_PAD_X))
        .pt(px(HUD_PAD_TOP_NOTCH))
        .pb(px(HUD_PAD_BOTTOM_NOTCH))
        .flex()
        .flex_col()
        .gap(px(6.0))
        .shadow(vec![BoxShadow {
            color: hsla(0.0, 0.0, 0.0, 0.28),
            offset: point(px(0.0), px(6.0)),
            blur_radius: px(20.0),
            spread_radius: px(0.0),
            inset: false,
        }])
}

fn mode_label(text: &str, color: u32) -> Div {
    div()
        .text_size(px(MICRO.size))
        .font_weight(FontWeight::SEMIBOLD)
        .font_family("SF Pro")
        .text_color(rgb(color))
        .child(text.to_uppercase())
}

/// The five level bars, 3 px wide, 3–14 px high in the mode colour.
fn level_bars(level: f32, color: u32) -> Div {
    const SHAPE: [f32; 5] = [0.55, 0.8, 1.0, 0.7, 0.9];
    div().flex().items_center().gap(px(2.0)).children(
        SHAPE
            .into_iter()
            .map(|f| {
                div()
                    .w(px(3.0))
                    .h(px(3.0 + level.clamp(0.0, 1.0) * 11.0 * f))
                    .rounded(px(2.0))
                    .bg(rgb(color))
            })
            .collect::<Vec<_>>(),
    )
}

/// The 12 px decoding ring. Static: the `decoding` text carries the state and
/// Reduce Motion stops the prototype's spin anyway (§8).
fn spinner() -> Div {
    div()
        .w(px(HUD_SPINNER))
        .h(px(HUD_SPINNER))
        .rounded_full()
        .border_2()
        .border_color(rgb(HUD_TOKENS.soft))
}

/// Right-aligned meta: 11 px mono, soft; the countdown reads in text colour.
fn meta(text: &str, countdown: bool) -> Div {
    div()
        .text_size(px(11.0))
        .font_family("SF Mono")
        .text_color(rgb(if countdown {
            HUD_TOKENS.text
        } else {
            HUD_TOKENS.soft
        }))
        .whitespace_nowrap()
        .child(text.to_string())
}

fn chip(text: &str, color: u32) -> Div {
    div()
        .bg(rgb(HUD_TOKENS.chip))
        .rounded(px(6.0))
        .px(px(8.0))
        .py(px(2.0))
        .text_size(px(CAPTION.size))
        .font_family("SF Pro")
        .text_color(rgb(color))
        .child(text.to_string())
}

/// Render one state as an island. Layouts follow the prototype: a mode row
/// (mode, bars/spinner, meta) with a load bar and detail lines under it; a
/// chip row for results and failures; a plain text row for the no-match and
/// download lines.
pub fn hud_island(state: &HudState) -> Div {
    let model = render_model(state);
    let mut island = island_base();
    match state {
        HudState::ListeningDictate { .. }
        | HudState::ListeningCommand { .. }
        | HudState::Decoding { .. }
        | HudState::Confirm { .. } => {
            let mut row = div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(mode_label(&model.primary_text, accent_text(model.accent)));
            row = match model.icon {
                HudIcon::Level(level) => row.child(level_bars(level, accent_text(model.accent))),
                HudIcon::Spinner => row.child(spinner()),
                HudIcon::None => row,
            };
            if let Some(meta_text) = &model.secondary_text {
                let countdown = meta_text.ends_with("left");
                row = row.child(div().flex_1()).child(meta(meta_text, countdown));
            }
            island = island.child(row);
            if state_matches_loading(state) {
                if let Some(progress) = model.progress {
                    island = island.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .flex_1()
                                    .h(px(HUD_BAR_H))
                                    .rounded_full()
                                    .bg(rgb(HUD_TOKENS.chip))
                                    .child(
                                        div()
                                            .h(px(HUD_BAR_H))
                                            .rounded_full()
                                            .bg(rgb(HUD_TOKENS.dictate))
                                            .w(DefiniteLength::Fraction(progress.clamp(0.0, 1.0))),
                                    ),
                            )
                            .child(meta("loading model while you speak", false)),
                    );
                }
            }
            for (n, line) in detail_lines(state).into_iter().enumerate() {
                let line = if n == 0 && state_is_listening(state) {
                    tail_text(&line, PARTIAL_TAIL_CHARS).to_string()
                } else {
                    line
                };
                let note = n > 0;
                let placeholder = line == "listening…";
                island = island.child(
                    div()
                        .text_size(px(if note { CAPTION.size } else { BODY.size }))
                        .font_family("SF Pro")
                        .text_color(rgb(if note || placeholder {
                            HUD_TOKENS.soft
                        } else {
                            HUD_TOKENS.text
                        }))
                        .child(line),
                );
            }
        }
        HudState::NoMatch { .. } | HudState::ModelDownloading { .. } => {
            island = island.child(
                div().flex().items_center().gap(px(10.0)).child(
                    div()
                        .text_size(px(BODY.size))
                        .font_family("SF Pro")
                        .text_color(rgb(HUD_TOKENS.text))
                        .child(model.primary_text),
                ),
            );
        }
        _ => {
            let mut row = div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(chip(&model.primary_text, accent_text(model.accent)));
            if let Some(meta_text) = &model.secondary_text {
                row = row.child(meta(meta_text, false));
            }
            island = island.child(row);
        }
    }
    island
}

/// The thin load bar only draws under a listening row (H3). The download
/// state stays a plain text line, as the prototype does.
fn state_matches_loading(state: &HudState) -> bool {
    matches!(
        state,
        HudState::ListeningDictate {
            loading: Some(_),
            ..
        } | HudState::ListeningCommand {
            loading: Some(_),
            ..
        }
    )
}

fn state_is_listening(state: &HudState) -> bool {
    matches!(
        state,
        HudState::ListeningDictate { .. } | HudState::ListeningCommand { .. }
    )
}
