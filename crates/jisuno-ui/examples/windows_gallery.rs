//! Windows gallery: the popover, Settings (switchable panes) and Welcome
//! windows with sample models, in light and dark.
//!
//! One window per view. The `Light`/`Dark` button flips the palette; the
//! Settings toolbar switches panes and the in-view sidebar works too.
//! Intents print to stderr; this is the host stub.
//!
//! ```sh
//! cargo run -p jisuno-ui --example windows_gallery
//! JISUNO_GALLERY_VIEW=settings cargo run -p jisuno-ui --example windows_gallery
//! ```
//!
//! `JISUNO_GALLERY_VIEW` is `popover`, `settings`, `welcome` or unset (all
//! three windows). `JISUNO_GALLERY_PANE` selects the initial Settings pane.

#[cfg(target_os = "macos")]
mod gallery {
    use std::rc::Rc;

    use std::time::Duration;

    use gpui::TitlebarOptions;
    use gpui::{
        div, prelude::*, px, rgb, size, App, AsyncApp, Bounds, Context, Entity, MouseButton,
        MouseDownEvent, WeakEntity, Window, WindowBounds, WindowOptions,
    };
    use jisuno_ui::popover_model::{PopoverIntent, PopoverModel};
    use jisuno_ui::popover_view::{popover_view, PopoverEmit};
    use jisuno_ui::settings_model::{Pane, SettingsIntent, SettingsModel};
    use jisuno_ui::settings_view::{settings_view, SettingsEmit};
    use jisuno_ui::tokens::{Theme, WELCOME_H, WELCOME_W};
    use jisuno_ui::welcome_model::{WelcomeIntent, WelcomeModel};
    use jisuno_ui::welcome_view::{welcome_view, WelcomeEmit};

    fn theme(dark: bool) -> Theme {
        if dark {
            Theme::quiet_dark()
        } else {
            Theme::quiet_light()
        }
    }

    fn theme_button(
        dark: bool,
        toggle: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> gpui::Div {
        div()
            .px(px(10.0))
            .py(px(4.0))
            .rounded(px(6.0))
            .bg(rgb(theme(dark).palette.chip))
            .text_size(px(12.0))
            .font_family("SF Pro")
            .text_color(rgb(theme(dark).palette.text))
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, toggle)
            .child(if dark { "Dark" } else { "Light" })
    }

    struct PopoverWindow {
        models: Vec<PopoverModel>,
        index: usize,
        dark: bool,
    }

    impl PopoverWindow {
        fn emit(&self, entity: Entity<Self>) -> PopoverEmit {
            Rc::new(
                move |intent: PopoverIntent, _window: &mut Window, cx: &mut App| {
                    eprintln!("popover intent: {intent:?}");
                    entity.update(cx, |this, cx| {
                        this.apply(intent, cx);
                    });
                },
            )
        }

        fn apply(&mut self, intent: PopoverIntent, cx: &mut Context<Self>) {
            if let PopoverIntent::CopyRecent(index) = intent {
                if let Some(item) = self.models[self.index].recent_rows().get(index) {
                    eprintln!("copy: {}", item.display_text());
                }
                self.models[self.index].toast = Some("Copied".into());
            }
            cx.notify();
        }
    }

    impl Render for PopoverWindow {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let index = self.index;
            let count = self.models.len();
            let model = self.models[index % count.max(1)].clone();
            let theme = theme(self.dark);
            let dark = self.dark;
            let advance = cx.listener(|this: &mut Self, _: &MouseDownEvent, _, cx| {
                this.index = (this.index + 1) % this.models.len().max(1);
                this.models[this.index].toast = None;
                cx.notify();
            });
            let toggle = cx.listener(|this: &mut Self, _: &MouseDownEvent, _, cx| {
                this.dark = !this.dark;
                cx.notify();
            });
            div()
                .flex()
                .flex_col()
                .size_full()
                .bg(rgb(theme.palette.ground))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(12.0))
                        .py(px(8.0))
                        .text_size(px(12.0))
                        .font_family("SF Pro")
                        .text_color(rgb(theme.palette.soft))
                        .child(format!("popover {}/{} · click advances", index + 1, count))
                        .child(theme_button(dark, toggle)),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_start()
                        .justify_center()
                        .py(px(16.0))
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, advance)
                        .child(popover_view(&model, &theme, self.emit(cx.entity()))),
                )
        }
    }

    struct SettingsWindow {
        model: SettingsModel,
        dark: bool,
    }

    impl SettingsWindow {
        fn emit(&self, entity: Entity<Self>) -> SettingsEmit {
            Rc::new(
                move |intent: SettingsIntent, _window: &mut Window, cx: &mut App| {
                    eprintln!("settings intent: {intent:?}");
                    entity.update(cx, |this, cx| {
                        this.apply(intent, cx);
                    });
                },
            )
        }

        fn apply(&mut self, intent: SettingsIntent, cx: &mut Context<Self>) {
            match intent {
                SettingsIntent::SelectPane(pane) => {
                    self.model.pane = pane;
                }
                SettingsIntent::SetSetting { key, value, .. } => {
                    eprintln!("host would write {key} = {value}");
                }
                SettingsIntent::StartRecording { command, when } => {
                    self.model.recording = Some(jisuno_ui::settings_model::Recording {
                        command,
                        when,
                        live_display: "Press keys…".into(),
                    });
                }
                SettingsIntent::CancelRecording => {
                    self.model.recording = None;
                    self.model.pending_conflict = None;
                }
                SettingsIntent::ReplaceBinding => {
                    if let Some(pending) = self.model.pending_conflict.take() {
                        let binding = jisuno_logic::keybindings::Binding {
                            key: pending.key,
                            hold: false,
                            when: pending.when,
                            command: self
                                .model
                                .recording
                                .as_ref()
                                .map(|r| r.command)
                                .unwrap_or(pending.taken_by),
                        };
                        let _ = binding;
                    }
                    self.model.recording = None;
                }
                SettingsIntent::ResetBinding(_) => {}
                _ => {}
            }
            cx.notify();
        }
    }

    impl Render for SettingsWindow {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = theme(self.dark);
            let dark = self.dark;
            let toggle = cx.listener(|this: &mut Self, _: &MouseDownEvent, _, cx| {
                this.dark = !this.dark;
                cx.notify();
            });
            let current = self.model.pane;
            let pills =
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap(px(4.0))
                    .children(Pane::all().iter().map(|pane| {
                        let pane = *pane;
                        let selected = pane == current;
                        let switch =
                            cx.listener(move |this: &mut Self, _: &MouseDownEvent, _, cx| {
                                this.model.pane = pane;
                                cx.notify();
                            });
                        let mut pill = div()
                            .px(px(8.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .text_size(px(12.0))
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
                                .on_mouse_down(MouseButton::Left, switch);
                        }
                        pill.child(pane.title().to_string())
                    }));
            div()
                .flex()
                .flex_col()
                .size_full()
                .bg(rgb(theme.palette.ground))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(12.0))
                        .py(px(8.0))
                        .text_size(px(12.0))
                        .font_family("SF Pro")
                        .text_color(rgb(theme.palette.soft))
                        .child("settings · pick a pane".to_string())
                        .child(theme_button(dark, toggle))
                        .child(pills),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_start()
                        .justify_center()
                        .py(px(12.0))
                        .child(settings_view(&self.model, &theme, self.emit(cx.entity()))),
                )
        }
    }

    struct WelcomeWindow {
        models: Vec<WelcomeModel>,
        index: usize,
        dark: bool,
    }

    impl WelcomeWindow {
        fn emit(&self, entity: Entity<Self>) -> WelcomeEmit {
            Rc::new(
                move |intent: WelcomeIntent, _window: &mut Window, cx: &mut App| {
                    eprintln!("welcome intent: {intent:?}");
                    entity.update(cx, |this, cx| {
                        this.apply(intent, cx);
                    });
                },
            )
        }

        fn apply(&mut self, intent: WelcomeIntent, cx: &mut Context<Self>) {
            match intent {
                WelcomeIntent::DownloadModel => {
                    let model = &mut self.models[self.index];
                    if model.download.is_none() && !model.model_present {
                        model.download = Some(jisuno_ui::welcome_model::DownloadProgress {
                            percent: 0,
                            downloaded_mb: 0,
                            total_mb: jisuno_ui::welcome_model::MODEL_TOTAL_MB,
                        });
                        cx.spawn(
                            async move |weak: WeakEntity<WelcomeWindow>, cx: &mut AsyncApp| {
                                for percent in (1..=100u32).step_by(5) {
                                    cx.background_executor()
                                        .timer(Duration::from_millis(120))
                                        .await;
                                    let alive = weak
                                        .update(cx, |this: &mut WelcomeWindow, cx| {
                                            let at = this.index;
                                            let model = &mut this.models[at];
                                            if let Some(download) = &mut model.download {
                                                download.percent = percent;
                                                download.downloaded_mb =
                                                    percent * download.total_mb / 100;
                                                if percent >= 100 {
                                                    model.download = None;
                                                    model.model_present = true;
                                                }
                                                cx.notify();
                                            }
                                        })
                                        .is_ok();
                                    if !alive {
                                        break;
                                    }
                                }
                            },
                        )
                        .detach();
                    }
                }
                WelcomeIntent::OpenPermissionPane(_) => {}
                WelcomeIntent::Start => {}
            }
            cx.notify();
        }
    }

    impl Render for WelcomeWindow {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let index = self.index;
            let count = self.models.len();
            let model = self.models[index % count.max(1)].clone();
            let theme = theme(self.dark);
            let dark = self.dark;
            let advance = cx.listener(|this: &mut Self, _: &MouseDownEvent, _, cx| {
                this.index = (this.index + 1) % this.models.len().max(1);
                cx.notify();
            });
            let toggle = cx.listener(|this: &mut Self, _: &MouseDownEvent, _, cx| {
                this.dark = !this.dark;
                cx.notify();
            });
            div()
                .flex()
                .flex_col()
                .size_full()
                .bg(rgb(theme.palette.ground))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(12.0))
                        .py(px(8.0))
                        .text_size(px(12.0))
                        .font_family("SF Pro")
                        .text_color(rgb(theme.palette.soft))
                        .child(format!("welcome {}/{} · click advances", index + 1, count))
                        .child(theme_button(dark, toggle)),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_start()
                        .justify_center()
                        .py(px(12.0))
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, advance)
                        .child(welcome_view(&model, &theme, self.emit(cx.entity()))),
                )
        }
    }

    pub fn run() {
        let only = std::env::var("JISUNO_GALLERY_VIEW").unwrap_or_default();
        let show_popover = only.is_empty() || only == "popover";
        let show_settings = only.is_empty() || only == "settings";
        let show_welcome = only.is_empty() || only == "welcome";
        let dark = std::env::var("JISUNO_GALLERY_DARK").is_ok();
        let initial_pane = std::env::var("JISUNO_GALLERY_PANE")
            .ok()
            .and_then(|name| Pane::all().into_iter().find(|p| p.title() == name));
        gpui_platform::application().run(move |cx: &mut App| {
            if let Err(error) = jisuno_ui::hud_view::load_system_fonts(cx) {
                eprintln!("windows_gallery: {error:#}");
            }
            if show_popover {
                let bounds = Bounds::centered(None, size(px(420.0), px(640.0)), cx);
                cx.open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        titlebar: Some(TitlebarOptions {
                            title: Some("Popover".into()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    |_, cx| {
                        cx.new(|_| PopoverWindow {
                            models: vec![PopoverModel::sample(), PopoverModel::sample_attention()],
                            index: 0,
                            dark,
                        })
                    },
                )
                .unwrap();
            }
            if show_settings {
                let bounds = Bounds::centered(None, size(px(880.0), px(700.0)), cx);
                cx.open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        titlebar: Some(TitlebarOptions {
                            title: Some("Settings".into()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    |_, cx| {
                        cx.new(|_| {
                            let mut model = SettingsModel::sample();
                            if let Some(pane) = initial_pane {
                                model.pane = pane;
                            }
                            SettingsWindow { model, dark }
                        })
                    },
                )
                .unwrap();
            }
            if show_welcome {
                let bounds =
                    Bounds::centered(None, size(px(WELCOME_W + 80.0), px(WELCOME_H + 120.0)), cx);
                cx.open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        titlebar: Some(TitlebarOptions {
                            title: Some("Welcome to Jisuno".into()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    |_, cx| {
                        cx.new(|_| WelcomeWindow {
                            models: vec![
                                WelcomeModel::sample(),
                                WelcomeModel::sample_downloading(),
                            ],
                            index: 0,
                            dark,
                        })
                    },
                )
                .unwrap();
            }
            cx.activate(true);
        });
    }
}

#[cfg(target_os = "macos")]
fn main() {
    gallery::run();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("windows_gallery needs macOS: GPUI is macOS-only in jisuno-ui.");
}
