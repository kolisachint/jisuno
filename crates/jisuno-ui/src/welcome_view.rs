//! The first-run Welcome window view (macOS only): renders a [`WelcomeModel`]
//! at the sizes in `app-ui.md` §§1/6. System fonts.
//!
//! The view emits [`WelcomeIntent`]s through the callback; the host checks
//! permissions live, runs the download and writes the config files.

use std::rc::Rc;

use gpui::{div, prelude::*, px, rgb, Div, FontWeight};

use crate::tokens::{
    Theme, BODY, CAPTION, CONTROL_MIN_H, DISPLAY, LABEL, RADIUS_CONTROL, RADIUS_WINDOW, TITLE,
    WELCOME_H, WELCOME_W,
};
use crate::welcome_model::{PermissionKind, WelcomeIntent, WelcomeModel, PERMISSIONS};

/// Host callback: receives the intent plus the GPUI window/app so the
/// host can update its state (`entity.update(cx, …)`) and perform the
/// file/OS side of the intent.
pub type WelcomeEmit = Rc<dyn Fn(WelcomeIntent, &mut gpui::Window, &mut gpui::App)>;

/// A tap handler for plain buttons (no intent of its own).
pub type WelcomeTap = Rc<dyn Fn(&mut gpui::Window, &mut gpui::App)>;

fn body_text(text: String, theme: &Theme) -> Div {
    div()
        .text_size(px(BODY.size))
        .font_family("SF Pro")
        .text_color(rgb(theme.palette.text))
        .child(text)
}

fn caption(text: String, theme: &Theme) -> Div {
    div()
        .text_size(px(CAPTION.size))
        .font_family("SF Pro")
        .text_color(rgb(theme.palette.soft))
        .child(text)
}

fn button(
    label: &str,
    theme: &Theme,
    primary: bool,
    enabled: bool,
    on_tap: Option<WelcomeTap>,
) -> Div {
    let mut b = div()
        .min_h(px(CONTROL_MIN_H))
        .px(px(12.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS_CONTROL))
        .text_size(px(LABEL.size))
        .font_family("SF Pro");
    if primary && enabled {
        b = b
            .bg(rgb(theme.dictate_fill))
            .text_color(rgb(theme.button_label));
    } else {
        b = b
            .bg(rgb(theme.palette.chip))
            .text_color(rgb(theme.palette.text));
    }
    if !enabled {
        return b.child(label.to_string());
    }
    if let Some(on_tap) = on_tap {
        b = b.cursor_pointer().on_mouse_down(
            gpui::MouseButton::Left,
            move |_, window: &mut gpui::Window, cx: &mut gpui::App| {
                on_tap(window, cx);
            },
        );
    }
    b.child(label.to_string())
}

fn ok_color(theme: &Theme) -> u32 {
    if theme.dark {
        0x58c48a
    } else {
        0x2a7b4d
    }
}

fn warn_color(theme: &Theme) -> u32 {
    if theme.dark {
        0xe0a23c
    } else {
        0x9e5f09
    }
}

fn step_header(n: &str, title: &str, theme: &Theme) -> Div {
    div()
        .pt(px(4.0))
        .text_size(px(TITLE.size))
        .font_weight(FontWeight::SEMIBOLD)
        .font_family("SF Pro")
        .text_color(rgb(theme.palette.text))
        .child(format!("{n} · {title}"))
}

/// Render the Welcome window. `on_intent` receives every interaction.
pub fn welcome_view(model: &WelcomeModel, theme: &Theme, on_intent: WelcomeEmit) -> Div {
    let mut root = div()
        .w(px(WELCOME_W))
        .h(px(WELCOME_H))
        .bg(rgb(theme.palette.bg))
        .rounded(px(RADIUS_WINDOW))
        .px(px(22.0))
        .py(px(12.0))
        .flex()
        .flex_col()
        .child(
            div()
                .text_size(px(DISPLAY.size))
                .font_weight(FontWeight::SEMIBOLD)
                .font_family("SF Pro")
                .text_color(rgb(theme.palette.text))
                .child("Welcome to Jisuno"),
        )
        .child(caption(
            "Hold a key, speak, let go. Three things to set up first.".into(),
            theme,
        ))
        .child(step_header("1", "Permissions", theme));
    for (index, kind) in PERMISSIONS.iter().enumerate() {
        root = root.child(permission_row(
            *kind,
            model.allowed[index],
            theme,
            &on_intent,
        ));
    }
    root = root
        .child(caption(
            "Checked live: this window updates when System Settings changes.".into(),
            theme,
        ))
        .child(step_header("2", "Config files", theme))
        .child(
            div()
                .rounded(px(RADIUS_CONTROL))
                .px(px(10.0))
                .py(px(4.0))
                .bg(rgb(theme.palette.ground))
                .child(
                    div()
                        .text_size(px(BODY.size))
                        .font_family("SF Pro")
                        .text_color(rgb(ok_color(theme)))
                        .child(if model.files_written {
                            format!("Written to {}", model.config_path)
                        } else {
                            format!("Writing to {}…", model.config_path)
                        }),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_family("SF Mono")
                        .text_color(rgb(theme.palette.text))
                        .child("settings.json · keybindings.json · theme.json · voice-control-config.json · schemas/"),
                ),
        )
        .child(step_header("3", "Speech model", theme))
        .child(body_text(
            "Parakeet TDT 0.6B v3, int8 · about 650 MB".into(),
            theme,
        ));
    let (tone, line) = model.model_line();
    let line_color = match tone {
        Some("ok") => ok_color(theme),
        _ => theme.palette.text,
    };
    root = root.child(
        div()
            .text_size(px(BODY.size))
            .font_family("SF Pro")
            .text_color(rgb(line_color))
            .child(line),
    );
    if let Some(download) = &model.download {
        root = root.child(progress_bar(download.percent, theme));
        root = root.child(caption(
            "Dictation works once the download finishes.".into(),
            theme,
        ));
    }
    let on_intent_dl = on_intent.clone();
    root = root.child(div().flex().items_center().gap(px(8.0)).child({
        let enabled = model.download.is_none() && !model.model_present;
        let tap: Option<WelcomeTap> = enabled.then(|| {
            let tap: WelcomeTap = Rc::new(move |window: &mut gpui::Window, cx: &mut gpui::App| {
                on_intent_dl(WelcomeIntent::DownloadModel, window, cx);
            });
            tap
        });
        button("Download", theme, false, enabled, tap)
    }));
    let on_intent_start = on_intent.clone();
    let start_tap: Option<WelcomeTap> = model.can_start().then(|| {
        let tap: WelcomeTap = Rc::new(move |window: &mut gpui::Window, cx: &mut gpui::App| {
            on_intent_start(WelcomeIntent::Start, window, cx);
        });
        tap
    });
    root.child(
        div()
            .flex()
            .items_center()
            .justify_end()
            .gap(px(10.0))
            .pt(px(6.0))
            .children(
                model
                    .start_hint()
                    .map(|hint| caption(hint.to_string(), theme)),
            )
            .child(button(
                "Start Jisuno",
                theme,
                true,
                model.can_start(),
                start_tap,
            )),
    )
}

fn permission_row(
    kind: PermissionKind,
    allowed: bool,
    theme: &Theme,
    on_intent: &WelcomeEmit,
) -> Div {
    let mut row = div()
        .flex()
        .justify_between()
        .gap(px(16.0))
        .py(px(3.0))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .child(body_text(kind.name().to_string(), theme))
                .child(caption(kind.reason().to_string(), theme)),
        );
    if allowed {
        row = row.child(
            div().flex().items_center().child(
                div()
                    .text_size(px(BODY.size))
                    .font_family("SF Pro")
                    .text_color(rgb(ok_color(theme)))
                    .child("Allowed"),
            ),
        );
    } else {
        let on_intent = on_intent.clone();
        let tap: WelcomeTap = Rc::new(move |window: &mut gpui::Window, cx: &mut gpui::App| {
            on_intent(WelcomeIntent::OpenPermissionPane(kind), window, cx);
        });
        row = row.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(BODY.size))
                        .font_family("SF Pro")
                        .text_color(rgb(warn_color(theme)))
                        .child("Not allowed"),
                )
                .child(button(
                    "Open System Settings",
                    theme,
                    false,
                    true,
                    Some(tap),
                )),
        );
    }
    row
}

fn progress_bar(percent: u32, theme: &Theme) -> Div {
    div()
        .h(px(3.0))
        .rounded_full()
        .bg(rgb(theme.palette.chip))
        .child(
            div()
                .h(px(3.0))
                .rounded_full()
                .bg(rgb(theme.dictate_fill))
                .w(gpui::DefiniteLength::Fraction(
                    f32::from(percent.min(100) as u16) / 100.0,
                )),
        )
}
