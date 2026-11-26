//! The menu bar popover view (macOS only): renders a [`PopoverModel`] at the
//! sizes in `app-ui.md` §§1/4. System fonts (SF Pro / SF Mono), like the HUD.
//!
//! The view emits [`PopoverIntent`]s through the callback; the host performs
//! them and feeds updated models back.

use std::rc::Rc;

use gpui::{div, prelude::*, px, rgb, Div, FontWeight};

use crate::popover_model::{ModelState, PermissionPane, PopoverIntent, PopoverModel};
use crate::tokens::{
    Theme, BODY, CAPTION, CONTROL_MIN_H, MICRO, POPOVER_PAD, POPOVER_ROW_H, POPOVER_W,
    RADIUS_CONTROL, RADIUS_WINDOW,
};

fn tone(hex: &str) -> u32 {
    match hex {
        "ok" => 0x2a7b4d,
        "warning" => 0x9e5f09,
        "caution" => 0xc93a3a,
        _ => 0x686a6c,
    }
}

fn tone_dark(hex: &str) -> u32 {
    match hex {
        "ok" => 0x58c48a,
        "warning" => 0xe0a23c,
        "caution" => 0xff7b72,
        _ => 0x959596,
    }
}

fn tone_color(name: &str, dark: bool) -> u32 {
    if dark {
        tone_dark(name)
    } else {
        tone(name)
    }
}

fn section_label(text: &str, theme: &Theme) -> Div {
    div()
        .text_size(px(MICRO.size))
        .font_weight(FontWeight::MEDIUM)
        .font_family("SF Pro")
        .text_color(rgb(theme.palette.soft))
        .child(text.to_uppercase())
}

fn row(theme: &Theme, on_intent: Rc<dyn Fn(PopoverIntent)>, intent: Option<PopoverIntent>) -> Div {
    let mut row = div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .min_h(px(POPOVER_ROW_H))
        .px(px(8.0));
    if let Some(intent) = intent {
        let hover_bg = theme.palette.selected;
        row = row
            .cursor_pointer()
            .rounded(px(RADIUS_CONTROL))
            .hover(move |s| s.bg(rgb(hover_bg)))
            .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
                on_intent(intent.clone());
            });
    }
    row
}

fn body_text(text: String, theme: &Theme) -> Div {
    div()
        .text_size(px(BODY.size))
        .font_family("SF Pro")
        .text_color(rgb(theme.palette.text))
        .child(text)
}

fn caption_text(text: String, theme: &Theme) -> Div {
    div()
        .text_size(px(CAPTION.size))
        .font_family("SF Pro")
        .text_color(rgb(theme.palette.soft))
        .child(text)
}

/// Render the popover. `on_intent` receives every interaction.
pub fn popover_view(
    model: &PopoverModel,
    theme: &Theme,
    on_intent: Rc<dyn Fn(PopoverIntent)>,
) -> Div {
    let mut root = div()
        .w(px(POPOVER_W))
        .bg(rgb(theme.palette.popover))
        .rounded(px(RADIUS_WINDOW))
        .p(px(POPOVER_PAD))
        .flex()
        .flex_col()
        .gap(px(2.0));
    root = root.child(header(model, theme));
    root = root.child(model_section(model, theme, on_intent.clone()));
    root = root.child(permissions_section(model, theme, on_intent.clone()));
    if let Some(holder) = &model.secure_input_holder {
        root = root.child(secure_row(holder, theme));
    }
    root = root.child(recent_section(model, theme, on_intent.clone()));
    root = root.child(caption_text(
        "Recent is kept in memory until you quit".into(),
        theme,
    ));
    root = root.child(footer(theme, on_intent.clone()));
    if let Some(toast) = &model.toast {
        root = root.child(
            div()
                .bg(rgb(theme.palette.popover))
                .rounded(px(8.0))
                .px(px(12.0))
                .py(px(6.0))
                .text_size(px(12.0))
                .font_family("SF Pro")
                .text_color(rgb(theme.palette.text))
                .child(toast.clone()),
        );
    }
    root
}

fn dot(tone: &str, theme: &Theme) -> Div {
    div()
        .w(px(8.0))
        .h(px(8.0))
        .rounded_full()
        .bg(rgb(tone_color(tone, theme.dark)))
}

fn header(model: &PopoverModel, theme: &Theme) -> Div {
    let (tone, line) = model.model.line();
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .child(dot(tone, theme))
        .child(
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .font_family("SF Pro")
                        .text_color(rgb(theme.palette.text))
                        .child("Jisuno"),
                )
                .child(caption_text(model.model_name.clone(), theme))
                .child(caption_text(line, theme)),
        )
}

fn model_section(model: &PopoverModel, theme: &Theme, on_intent: Rc<dyn Fn(PopoverIntent)>) -> Div {
    let action = model.model.action_label();
    let intent = match &model.model.presence {
        crate::popover_model::ModelPresence::NotDownloaded => Some(PopoverIntent::DownloadModel),
        crate::popover_model::ModelPresence::Warm => Some(PopoverIntent::UnloadModel),
        crate::popover_model::ModelPresence::Cold => Some(PopoverIntent::WarmModel),
        _ => None,
    };
    let _ = action_enabled(&model.model);
    div()
        .flex()
        .flex_col()
        .child(section_label("Model", theme))
        .child(intended_row(model, theme, on_intent, intent, action))
}

fn action_enabled(state: &ModelState) -> bool {
    state.action_enabled()
}

fn intended_row(
    model: &PopoverModel,
    theme: &Theme,
    on_intent: Rc<dyn Fn(PopoverIntent)>,
    intent: Option<PopoverIntent>,
    action: &str,
) -> Div {
    let mut text = body_text(action.to_string(), theme);
    if intent.is_none() {
        text = body_text(format!("{} · {}", action, model.model.line().1), theme);
    }
    let mut r = row(theme, on_intent, intent);
    r = r.child(div().flex_1().child(text));
    if model.model.action_enabled() && action == "Warm model" {
        r = r.child(kbd("⌃⌥W", theme));
    }
    r
}

fn permissions_section(
    model: &PopoverModel,
    theme: &Theme,
    on_intent: Rc<dyn Fn(PopoverIntent)>,
) -> Div {
    let mut section = div()
        .flex()
        .flex_col()
        .child(section_label("Permissions", theme));
    for (pane, allowed) in model.permissions.rows() {
        section = section.child(permission_row(pane, allowed, theme, on_intent.clone()));
    }
    section
}

fn permission_row(
    pane: PermissionPane,
    allowed: bool,
    theme: &Theme,
    on_intent: Rc<dyn Fn(PopoverIntent)>,
) -> Div {
    if allowed {
        row(theme, on_intent, None)
            .child(
                div()
                    .flex_1()
                    .child(body_text(pane.name().to_string(), theme)),
            )
            .child(
                div()
                    .text_size(px(BODY.size))
                    .font_family("SF Pro")
                    .text_color(rgb(tone_color("ok", theme.dark)))
                    .child("Allowed"),
            )
    } else {
        row(
            theme,
            on_intent,
            Some(PopoverIntent::OpenPermissionPane(pane)),
        )
        .child(
            div()
                .flex_1()
                .child(body_text(pane.name().to_string(), theme)),
        )
        .child(
            div()
                .text_size(px(BODY.size))
                .font_family("SF Pro")
                .text_color(rgb(tone_color("warning", theme.dark)))
                .child("Not allowed · Open System Settings"),
        )
    }
}

fn secure_row(holder: &str, theme: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .min_h(px(POPOVER_ROW_H))
        .px(px(8.0))
        .child(
            div()
                .text_size(px(BODY.size))
                .font_family("SF Pro")
                .text_color(rgb(tone_color("caution", theme.dark)))
                .child(format!("Secure input on ({holder}) · keys paused")),
        )
}

fn recent_section(
    model: &PopoverModel,
    theme: &Theme,
    on_intent: Rc<dyn Fn(PopoverIntent)>,
) -> Div {
    let mut section = div()
        .flex()
        .flex_col()
        .child(section_label("Recent · memory only", theme));
    let rows = model.recent_rows();
    if rows.is_empty() {
        section = section.child(
            div()
                .px(px(8.0))
                .child(caption_text("Nothing yet.".into(), theme)),
        );
    }
    for (index, item) in rows.iter().enumerate() {
        let mut r = row(
            theme,
            on_intent.clone(),
            Some(PopoverIntent::CopyRecent(index)),
        )
        .min_h(px(CONTROL_MIN_H))
        .child(div().flex_1().child(body_text(item.display_text(), theme)));
        if let Some(tag) = item.tag() {
            r = r.child(
                div()
                    .rounded(px(4.0))
                    .px(px(6.0))
                    .py(px(2.0))
                    .text_size(px(11.0))
                    .font_family("SF Pro")
                    .text_color(rgb(theme.palette.soft))
                    .child(tag.to_string()),
            );
        }
        section = section.child(r);
    }
    section
}

fn footer(theme: &Theme, on_intent: Rc<dyn Fn(PopoverIntent)>) -> Div {
    div()
        .flex()
        .flex_col()
        .child(
            row(theme, on_intent.clone(), Some(PopoverIntent::OpenSettings))
                .child(div().flex_1().child(body_text("Settings…".into(), theme)))
                .child(kbd("⌃⌥,", theme)),
        )
        .child(
            row(theme, on_intent, Some(PopoverIntent::Quit))
                .child(div().flex_1().child(body_text("Quit Jisuno".into(), theme)))
                .child(kbd("⌘Q", theme)),
        )
}

fn kbd(text: &str, theme: &Theme) -> Div {
    div()
        .rounded(px(4.0))
        .px(px(6.0))
        .py(px(2.0))
        .text_size(px(11.0))
        .font_family("SF Mono")
        .text_color(rgb(theme.palette.soft))
        .child(text.to_string())
}
