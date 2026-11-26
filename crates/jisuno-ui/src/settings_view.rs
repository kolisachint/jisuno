//! The Settings window view (macOS only): renders a [`SettingsModel`] with
//! its eight panes at the sizes in `app-ui.md` §§1/5. System fonts.
//!
//! The view emits [`SettingsIntent`]s through the callback; the host writes
//! files and feeds updated models back. Key recording: the key cell enters
//! recording mode on click and the host feeds recorded keys back into
//! [`feed_recorded_key`](crate::settings_model::feed_recorded_key).

use std::rc::Rc;

use gpui::{div, prelude::*, px, rgb, Div, FontWeight};

use crate::settings_model::{
    AppearanceModel, AudioModel, FileRow, GeneralModel, KeybindingRow, ModelId, ModelPaneModel,
    Pane, PerfRow, SettingsIntent, SettingsModel,
};

/// Host callback: receives the intent plus the GPUI window/app so the
/// host can update its state (`entity.update(cx, …)`) and perform the
/// file/OS side of the intent.
pub type SettingsEmit = Rc<dyn Fn(SettingsIntent, &mut gpui::Window, &mut gpui::App)>;

/// A tap handler for plain buttons (no intent of its own).
pub type SettingsTap = Rc<dyn Fn(&mut gpui::Window, &mut gpui::App)>;
use crate::tokens::{
    Theme, BODY, CAPTION, CONTROL_MIN_H, KBD, LABEL, MICRO, RADIUS_CONTROL, RADIUS_WINDOW,
    SETTINGS_H, SETTINGS_PAD, SETTINGS_SIDEBAR_W, SETTINGS_W, TITLE,
};

fn label_text(text: String, theme: &Theme) -> Div {
    div()
        .text_size(px(LABEL.size))
        .font_weight(FontWeight::MEDIUM)
        .font_family("SF Pro")
        .text_color(rgb(theme.palette.text))
        .child(text)
}

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

fn mono(text: String, theme: &Theme) -> Div {
    div()
        .text_size(px(12.0))
        .font_family("SF Mono")
        .text_color(rgb(theme.palette.text))
        .child(text)
}

fn button(
    label: &str,
    theme: &Theme,
    primary: bool,
    enabled: bool,
    on_tap: Option<SettingsTap>,
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

fn intent_button(
    label: &str,
    theme: &Theme,
    primary: bool,
    enabled: bool,
    intent: SettingsIntent,
    on_intent: &SettingsEmit,
) -> Div {
    let on_intent = on_intent.clone();
    button(
        label,
        theme,
        primary,
        enabled,
        enabled.then(|| {
            let tap: SettingsTap = Rc::new(move |window: &mut gpui::Window, cx: &mut gpui::App| {
                on_intent(intent.clone(), window, cx);
            });
            tap
        }),
    )
}

fn field_row(label: &str, hint: Option<&str>, theme: &Theme, control: Div) -> Div {
    div()
        .flex()
        .justify_between()
        .gap(px(16.0))
        .py(px(3.0))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .child(label_text(label.to_string(), theme))
                .children(hint.map(|hint| caption(hint.to_string(), theme))),
        )
        .child(div().flex().items_center().child(control))
}

fn toggle(on: bool, theme: &Theme, file: &'static str, key: &str, on_intent: &SettingsEmit) -> Div {
    let on_intent = on_intent.clone();
    let key = key.to_string();
    let tap: SettingsTap = Rc::new(move |window: &mut gpui::Window, cx: &mut gpui::App| {
        on_intent(
            SettingsIntent::SetSetting {
                file,
                key: key.clone(),
                value: serde_json::Value::Bool(!on),
            },
            window,
            cx,
        );
    });
    button(if on { "On" } else { "Off" }, theme, on, true, Some(tap))
}

fn options(
    choices: &[(&str, &str)],
    current: &str,
    theme: &Theme,
    file: &'static str,
    key: &str,
    on_intent: &SettingsEmit,
) -> Div {
    div().flex().flex_row().flex_wrap().gap(px(6.0)).children(
        choices
            .iter()
            .map(|(value, label)| {
                let selected = *value == current;
                let on_intent = on_intent.clone();
                let value = value.to_string();
                let key = key.to_string();
                let mut pill = div()
                    .min_h(px(CONTROL_MIN_H))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .rounded(px(RADIUS_CONTROL))
                    .text_size(px(LABEL.size))
                    .font_family("SF Pro");
                if selected {
                    pill = pill
                        .bg(rgb(theme.dictate_fill))
                        .text_color(rgb(theme.button_label));
                } else {
                    pill = pill
                        .bg(rgb(theme.palette.chip))
                        .text_color(rgb(theme.palette.text))
                        .cursor_pointer()
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            move |_, window: &mut gpui::Window, cx: &mut gpui::App| {
                                on_intent(
                                    SettingsIntent::SetSetting {
                                        file,
                                        key: key.clone(),
                                        value: serde_json::Value::String(value.clone()),
                                    },
                                    window,
                                    cx,
                                );
                            },
                        );
                }
                pill.child(label.to_string())
            })
            .collect::<Vec<_>>(),
    )
}

fn stepper(
    value: u32,
    theme: &Theme,
    file: &'static str,
    key: &str,
    on_intent: &SettingsEmit,
) -> Div {
    let mk = |next: u32| {
        let on_intent = on_intent.clone();
        let key = key.to_string();
        let tap: SettingsTap = Rc::new(move |window: &mut gpui::Window, cx: &mut gpui::App| {
            on_intent(
                SettingsIntent::SetSetting {
                    file,
                    key: key.clone(),
                    value: serde_json::Value::from(next),
                },
                window,
                cx,
            );
        });
        tap
    };
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(button(
            "−",
            theme,
            false,
            value > 0,
            value.checked_sub(1).map(|next| mk(next)),
        ))
        .child(mono(value.to_string(), theme))
        .child(button("+", theme, false, true, Some(mk(value + 1))))
}

fn pane_title(title: &str, subtitle: &str, theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .text_size(px(TITLE.size))
                .font_weight(FontWeight::SEMIBOLD)
                .font_family("SF Pro")
                .text_color(rgb(theme.palette.text))
                .child(title.to_string()),
        )
        .child(caption(subtitle.to_string(), theme))
}

/// Render the Settings window. `on_intent` receives every interaction.
pub fn settings_view(model: &SettingsModel, theme: &Theme, on_intent: SettingsEmit) -> Div {
    div()
        .w(px(SETTINGS_W))
        .h(px(SETTINGS_H))
        .bg(rgb(theme.palette.bg))
        .rounded(px(RADIUS_WINDOW))
        .flex()
        .flex_row()
        .child(sidebar(model, theme, &on_intent))
        .child(
            div()
                .flex_1()
                .bg(rgb(theme.palette.bg))
                .p(px(SETTINGS_PAD))
                .child(pane_body(model, theme, &on_intent)),
        )
}

fn sidebar(model: &SettingsModel, theme: &Theme, on_intent: &SettingsEmit) -> Div {
    let all = Pane::all();
    div()
        .w(px(SETTINGS_SIDEBAR_W))
        .bg(rgb(theme.palette.ground))
        .py(px(12.0))
        .px(px(8.0))
        .flex()
        .flex_col()
        .gap(px(2.0))
        .children(all.iter().map(|pane| {
            let pane = *pane;
            let selected = pane == model.pane;
            let on_intent = on_intent.clone();
            let mut item = div()
                .min_h(px(CONTROL_MIN_H))
                .px(px(10.0))
                .flex()
                .items_center()
                .rounded(px(RADIUS_CONTROL))
                .text_size(px(BODY.size))
                .font_family("SF Pro");
            if selected {
                item = item
                    .bg(rgb(theme.palette.selected))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(theme.palette.text));
            } else {
                item = item
                    .text_color(rgb(theme.palette.text))
                    .cursor_pointer()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        move |_, window: &mut gpui::Window, cx: &mut gpui::App| {
                            on_intent(SettingsIntent::SelectPane(pane), window, cx);
                        },
                    );
            }
            item.child(pane.title().to_string())
        }))
}

fn pane_body(model: &SettingsModel, theme: &Theme, on_intent: &SettingsEmit) -> Div {
    match model.pane {
        Pane::General => general_pane(&model.general, theme, on_intent),
        Pane::Audio => audio_pane(&model.audio, theme, on_intent),
        Pane::Model => model_pane(&model.model, theme, on_intent),
        Pane::Keybindings => keybindings_pane(model, theme, on_intent),
        Pane::Commands => commands_pane(model, theme),
        Pane::Appearance => appearance_pane(&model.appearance, theme, on_intent),
        Pane::Performance => performance_pane(model, theme, on_intent),
        Pane::Files => files_pane(&model.files, theme, on_intent),
    }
}

fn general_pane(general: &GeneralModel, theme: &Theme, on_intent: &SettingsEmit) -> Div {
    div().flex().flex_col()
        .child(pane_title("General", "settings.json. Each change is written at once and hot-reloaded.", theme))
        .child(field_row("Listening cue", Some("Where the HUD appears. Off hides it; failures still show in the popover and icon."), theme,
            options(&[("notch", "Notch"), ("cursor", "Cursor"), ("off", "Off")], &general.hud, theme, "settings", "hud", on_intent)))
        .child(field_row("Sounds", Some("Short tone on start, stop and cancel."), theme,
            toggle(general.sounds, theme, "settings", "sounds", on_intent)))
        .child(field_row("Live partial text", Some("partials · shown once the model is warm"), theme,
            toggle(general.partials, theme, "settings", "partials", on_intent)))
        .child(field_row("Insert text by", Some("Paste keeps the clipboard; type is used for apps in insert_type_apps and when paste fails."), theme,
            options(&[("paste", "Paste"), ("type", "Type")], &general.insert, theme, "settings", "insert", on_intent)))
        .child(field_row("Apps that take typing", Some("insert_type_apps (bundle ids)"), theme,
            mono(if general.insert_type_apps.is_empty() { "none".into() } else { general.insert_type_apps.join(", ") }, theme)))
        .child(field_row("Restore clipboard after paste", Some("restore_clipboard"), theme,
            toggle(general.restore_clipboard, theme, "settings", "restore_clipboard", on_intent)))
        .child(field_row("Dictation limit", Some("dictation_limit_s · seconds. The HUD counts down the last 15 s."), theme,
            stepper(general.dictation_limit_s, theme, "settings", "dictation_limit_s", on_intent)))
        .child(field_row("Hands-free stops after silence", Some("hands_free_silence_ms · hold keys ignore this: key-up ends the utterance"), theme,
            stepper(general.hands_free_silence_ms, theme, "settings", "hands_free_silence_ms", on_intent)))
        .child(field_row("Launch at login", Some("login_item"), theme,
            toggle(general.login_item, theme, "settings", "login_item", on_intent)))
}

fn audio_pane(audio: &AudioModel, theme: &Theme, on_intent: &SettingsEmit) -> Div {
    let mut pane = div()
        .flex()
        .flex_col()
        .child(pane_title(
            "Audio",
            "Mic picker. Default is the system input.",
            theme,
        ))
        .child(field_row(
            "Input device",
            Some("input_device"),
            theme,
            div().flex().flex_col().gap(px(6.0)).children(
                audio
                    .devices
                    .iter()
                    .map(|device| {
                        let selected = *device == audio.input_device;
                        let on_intent = on_intent.clone();
                        let label = device.clone();
                        let device = device.clone();
                        let mut pill = div()
                            .min_h(px(CONTROL_MIN_H))
                            .px(px(10.0))
                            .flex()
                            .items_center()
                            .rounded(px(RADIUS_CONTROL))
                            .text_size(px(BODY.size))
                            .font_family("SF Pro");
                        if selected {
                            pill = pill
                                .bg(rgb(theme.dictate_fill))
                                .text_color(rgb(theme.button_label));
                        } else {
                            pill = pill
                                .bg(rgb(theme.palette.chip))
                                .text_color(rgb(theme.palette.text))
                                .cursor_pointer()
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    move |_, window: &mut gpui::Window, cx: &mut gpui::App| {
                                        on_intent(
                                            SettingsIntent::SetSetting {
                                                file: "settings",
                                                key: "input_device".into(),
                                                value: serde_json::Value::String(device.clone()),
                                            },
                                            window,
                                            cx,
                                        );
                                    },
                                );
                        }
                        pill.child(label)
                    })
                    .collect::<Vec<_>>(),
            ),
        ));
    if let Some(warning) = audio.bluetooth_warning() {
        pane = pane.child(notice(warning, tone_of(theme, "warning"), theme));
    }
    if let Some(note) = audio.missing_device_note() {
        pane = pane.child(notice(&note, tone_of(theme, "caution"), theme));
    }
    pane = pane.child(field_row(
        "Test the microphone",
        Some("Shows the level meter for two seconds. Nothing is recorded."),
        theme,
        intent_button(
            "Test",
            theme,
            false,
            !audio.testing_mic,
            SettingsIntent::TestMic,
            on_intent,
        ),
    ));
    pane.child(caption(
        "Devices come from the system. A missing device falls back to the system input with a HUD note.".into(),
        theme,
    ))
}

fn tone_of(theme: &Theme, name: &str) -> u32 {
    match (name, theme.dark) {
        ("warning", false) => 0x9e5f09,
        ("warning", true) => 0xe0a23c,
        ("caution", false) => 0xc93a3a,
        ("caution", true) => 0xff7b72,
        ("ok", false) => 0x2a7b4d,
        _ => 0x58c48a,
    }
}

fn notice(text: &str, color: u32, _theme: &Theme) -> Div {
    div()
        .rounded(px(RADIUS_CONTROL))
        .px(px(10.0))
        .py(px(8.0))
        .my(px(4.0))
        .text_size(px(BODY.size))
        .font_family("SF Pro")
        .text_color(rgb(color))
        .child(text.to_string())
}

fn model_pane(model: &ModelPaneModel, theme: &Theme, on_intent: &SettingsEmit) -> Div {
    let mut pane = div().flex().flex_col().child(pane_title(
        "Model",
        "One model is loaded at a time. Switching unloads the current one.",
        theme,
    ));
    for id in [ModelId::V3, ModelId::V2] {
        let selected = model.selected == id;
        let on_intent = on_intent.clone();
        let file_id = id.file_id().to_string();
        let mut card = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .rounded(px(RADIUS_CONTROL))
            .px(px(10.0))
            .py(px(8.0));
        if selected {
            card = card.bg(rgb(theme.palette.selected));
        } else {
            card = card.cursor_pointer().on_mouse_down(
                gpui::MouseButton::Left,
                move |_, window: &mut gpui::Window, cx: &mut gpui::App| {
                    on_intent(
                        SettingsIntent::SetSetting {
                            file: "settings",
                            key: "model".into(),
                            value: serde_json::Value::String(file_id.clone()),
                        },
                        window,
                        cx,
                    );
                },
            );
        }
        pane = pane.child(card.child(body_text(format!("{} · {}", id.label(), id.note()), theme)));
    }
    pane = pane.child(
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .py(px(6.0))
            .child(caption("State:".into(), theme))
            .child(body_text(model.state_line(), theme)),
    );
    if let Some(percent) = model.download_percent {
        pane = pane.child(progress_bar(percent, theme));
    }
    pane = pane.child(
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(px(6.0))
            .py(px(6.0))
            .child(intent_button(
                "Download",
                theme,
                true,
                !model.downloaded && model.download_percent.is_none(),
                SettingsIntent::DownloadModel(model.selected.file_id()),
                on_intent,
            ))
            .child(intent_button(
                "Warm now",
                theme,
                false,
                model.downloaded && !model.warm,
                SettingsIntent::WarmModel,
                on_intent,
            ))
            .child(intent_button(
                "Unload now",
                theme,
                false,
                model.warm,
                SettingsIntent::UnloadModel,
                on_intent,
            )),
    );
    pane.child(field_row(
        "Load",
        Some("model_load"),
        theme,
        options(
            &[("on_demand", "On demand"), ("at_login", "At login")],
            &model.load_policy,
            theme,
            "settings",
            "model_load",
            on_intent,
        ),
    ))
    .child(field_row(
        "Unload after idle (min)",
        Some("unload_after_idle_min · 0 = never"),
        theme,
        stepper(
            model.unload_after_idle_min,
            theme,
            "settings",
            "unload_after_idle_min",
            on_intent,
        ),
    ))
    .child(field_row(
        "Language",
        Some("language · auto = the model detects"),
        theme,
        options(
            &[("auto", "auto")],
            &model.language,
            theme,
            "settings",
            "language",
            on_intent,
        ),
    ))
}

fn progress_bar(percent: u32, theme: &Theme) -> Div {
    div()
        .flex_1()
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

fn keybindings_pane(model: &SettingsModel, theme: &Theme, on_intent: &SettingsEmit) -> Div {
    let mut pane = div().flex().flex_col().child(pane_title(
        "Keybindings",
        "keybindings.json. Click a key, then press the new keys. Esc cancels. Save is immediate.",
        theme,
    ));
    for row in &model.keybinding_rows {
        pane = pane.child(keybinding_row_view(model, row, theme, on_intent));
    }
    pane
}

fn keybinding_row_view(
    model: &SettingsModel,
    row: &KeybindingRow,
    theme: &Theme,
    on_intent: &SettingsEmit,
) -> Div {
    let recording = model
        .recording
        .as_ref()
        .is_some_and(|r| r.command == row.command && r.when == row.when);
    let key_copy = if recording {
        model
            .recording
            .as_ref()
            .map(|r| r.live_display.clone())
            .unwrap_or_else(|| "Press keys…".into())
    } else {
        row.key_display()
    };
    let cell_intent = on_intent.clone();
    let (command, when) = (row.command, row.when);
    let key_cell = div()
        .min_h(px(CONTROL_MIN_H))
        .px(px(10.0))
        .flex()
        .items_center()
        .rounded(px(4.0))
        .bg(rgb(theme.palette.chip))
        .text_size(px(KBD.size))
        .font_family("SF Mono")
        .text_color(rgb(if recording {
            theme.dictate_text
        } else {
            theme.palette.text
        }))
        .cursor_pointer()
        .on_mouse_down(
            gpui::MouseButton::Left,
            move |_, window: &mut gpui::Window, cx: &mut gpui::App| {
                cell_intent(SettingsIntent::StartRecording { command, when }, window, cx);
            },
        )
        .child(key_copy);
    let on_intent = on_intent.clone();
    let mut pane = div().flex().flex_col().py(px(4.0)).child(
        div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .w(px(190.0))
                    .child(mono(row.command.id().to_string(), theme)),
            )
            .child(div().w(px(120.0)).child(key_cell))
            .child(
                div()
                    .w(px(80.0))
                    .child(caption(row.when_display().to_string(), theme)),
            )
            .child(div().w(px(40.0)).child(caption(
                if row.hold {
                    "hold".into()
                } else {
                    String::new()
                },
                theme,
            )))
            .child({
                let on_intent = on_intent.clone();
                div()
                    .text_size(px(BODY.size))
                    .font_family("SF Pro")
                    .text_color(rgb(theme.dictate_text))
                    .cursor_pointer()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        move |_, window: &mut gpui::Window, cx: &mut gpui::App| {
                            on_intent(SettingsIntent::ResetBinding(command), window, cx);
                        },
                    )
                    .child("Set to default")
            }),
    );
    if let Some(conflict) = &model.pending_conflict {
        if recording {
            let taken = conflict.taken_by_line();
            let on_intent = on_intent.clone();
            pane = pane.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(body_text(taken, theme))
                    .child(intent_button(
                        "Replace",
                        theme,
                        true,
                        true,
                        SettingsIntent::ReplaceBinding,
                        &on_intent,
                    ))
                    .child(intent_button(
                        "Cancel",
                        theme,
                        false,
                        true,
                        SettingsIntent::CancelRecording,
                        &on_intent,
                    )),
            );
        }
    }
    if let Some((command, message)) = &model.keybinding_error {
        if *command == row.command {
            pane = pane.child(notice(message, tone_of(theme, "caution"), theme));
        }
    }
    if let Some((command, warning)) = &model.keybinding_warning {
        if *command == row.command {
            pane = pane.child(notice(warning, tone_of(theme, "warning"), theme));
        }
    }
    pane
}

fn commands_pane(model: &SettingsModel, theme: &Theme) -> Div {
    let mut pane = div()
        .flex()
        .flex_col()
        .child(pane_title(
            "Commands",
            "voice-control-config.json. Read-only here: edit the file or use jisuno control.",
            theme,
        ))
        .child(section_header("Built in", theme));
    for row in &model.builtin_commands {
        pane = pane.child(command_row_view(
            &row.say.join(", "),
            &row.steps,
            row.confirm,
            theme,
        ));
    }
    pane = pane.child(section_header(
        &format!("Added by agents ({})", model.agent_commands.len()),
        theme,
    ));
    if model.agent_commands.is_empty() {
        pane = pane.child(caption("None.".into(), theme));
    }
    for row in &model.agent_commands {
        pane = pane.child(command_row_view(
            &row.say.join(", "),
            &row.steps,
            row.confirm,
            theme,
        ));
    }
    pane = pane.child(section_header("Workspaces", theme));
    for window in &model.workspaces {
        pane = pane.child(body_text(
            format!(
                "{} · {} · {} · {}",
                window.workspace, window.app, window.opens, window.place
            ),
            theme,
        ));
    }
    pane
}

fn section_header(text: &str, theme: &Theme) -> Div {
    div()
        .pt(px(10.0))
        .text_size(px(MICRO.size))
        .font_weight(FontWeight::MEDIUM)
        .font_family("SF Pro")
        .text_color(rgb(theme.palette.soft))
        .child(text.to_uppercase())
}

fn command_row_view(say: &str, steps: &str, confirm: bool, theme: &Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .py(px(4.0))
        .child(div().flex_1().child(body_text(format!("\"{say}\""), theme)))
        .child(div().flex_1().child(mono(steps.to_string(), theme)))
        .children(confirm.then(|| {
            div()
                .rounded(px(4.0))
                .px(px(6.0))
                .py(px(2.0))
                .text_size(px(11.0))
                .font_family("SF Pro")
                .text_color(rgb(theme.palette.soft))
                .child("confirm")
        }))
}

fn appearance_pane(appearance: &AppearanceModel, theme: &Theme, on_intent: &SettingsEmit) -> Div {
    div()
        .flex()
        .flex_col()
        .child(pane_title(
            "Appearance",
            "theme.json. Palette: inkuno Quiet. Accents are fills; text uses the read variants.",
            theme,
        ))
        .child(field_row(
            "Palette",
            Some("name · quiet-light or quiet-dark. Used when appearance is fixed."),
            theme,
            options(
                &[("quiet-light", "quiet-light"), ("quiet-dark", "quiet-dark")],
                &appearance.name,
                theme,
                "theme",
                "name",
                on_intent,
            ),
        ))
        .child(field_row(
            "Appearance",
            Some("system follows macOS; fixed uses name"),
            theme,
            options(
                &[("system", "system"), ("fixed", "fixed")],
                &appearance.appearance,
                theme,
                "theme",
                "appearance",
                on_intent,
            ),
        ))
        .child(field_row(
            "Dictate accent",
            Some("accent_dictate · #rrggbb"),
            theme,
            mono(appearance.accent_dictate.clone(), theme),
        ))
        .child(field_row(
            "Command accent",
            Some("accent_command · #rrggbb"),
            theme,
            mono(appearance.accent_command.clone(), theme),
        ))
        .child(field_row(
            "HUD style",
            Some("hud.style · the only value in v1"),
            theme,
            mono(appearance.hud_style.clone(), theme),
        ))
        .child(field_row(
            "Show latency in HUD",
            Some("hud.show_latency · ms after the result line"),
            theme,
            toggle(
                appearance.show_latency,
                theme,
                "theme",
                "hud.show_latency",
                on_intent,
            ),
        ))
}

fn performance_pane(model: &SettingsModel, theme: &Theme, on_intent: &SettingsEmit) -> Div {
    let mut pane = div().flex().flex_col()
        .child(pane_title("Performance", "Targets from jisuno.md §6 and §6.1. Measured values come from jisuno bench on the reference Mac.", theme));
    pane = pane.child(perf_table(&model.perf, theme));
    pane = pane.child(caption(
        "Reference machine: MacBook Air M1, 8 GB, macOS 27.3. Numbers are recorded, not gated."
            .into(),
        theme,
    ));
    pane.child(div().py(px(6.0)).child(intent_button(
        if model.bench_running {
            "Running…"
        } else {
            "Run benchmark"
        },
        theme,
        false,
        !model.bench_running,
        SettingsIntent::RunBench,
        on_intent,
    )))
}

fn perf_table(rows: &[PerfRow], theme: &Theme) -> Div {
    let mut table = div().flex().flex_col().py(px(6.0)).child(
        div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(div().w(px(340.0)).child(section_header("Metric", theme)))
            .child(div().w(px(110.0)).child(section_header("Target", theme)))
            .child(div().flex_1().child(section_header("Measured", theme))),
    );
    for row in rows {
        table = table.child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .py(px(3.0))
                .child(
                    div()
                        .w(px(340.0))
                        .child(body_text(row.metric.to_string(), theme)),
                )
                .child(
                    div()
                        .w(px(110.0))
                        .child(mono(row.target.to_string(), theme)),
                )
                .child(
                    div()
                        .flex_1()
                        .child(caption(row.measured_display().to_string(), theme)),
                ),
        );
    }
    table
}

fn files_pane(files: &[FileRow], theme: &Theme, on_intent: &SettingsEmit) -> Div {
    let mut pane = div().flex().flex_col()
        .child(pane_title("Files", "Files are the settings. Open one in your editor, or copy its last error. Folder: ~/.config/jisuno/", theme));
    for file in files {
        let error = file.error.is_some();
        pane = pane.child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .py(px(4.0))
                .child(div().w(px(220.0)).child(mono(file.name.to_string(), theme)))
                .child(div().flex_1().child(caption(file.path.clone(), theme)))
                .child(
                    div()
                        .text_size(px(BODY.size))
                        .font_family("SF Pro")
                        .text_color(rgb(tone_of(theme, if error { "caution" } else { "ok" })))
                        .child(file.status()),
                ),
        );
        if let Some(message) = &file.error {
            pane = pane.child(notice(message, tone_of(theme, "caution"), theme));
        }
        let name = file.name;
        pane = pane.child(
            div()
                .flex()
                .flex_row()
                .gap(px(6.0))
                .py(px(2.0))
                .child(intent_button(
                    "Open file",
                    theme,
                    false,
                    true,
                    SettingsIntent::OpenFile(name),
                    on_intent,
                ))
                .child(intent_button(
                    "Copy error",
                    theme,
                    false,
                    error,
                    SettingsIntent::CopyError(name),
                    on_intent,
                )),
        );
    }
    pane
}
