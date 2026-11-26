//! HUD gallery: every [`HudState`] on both palettes, in one window.
//!
//! The island itself is always dark (§1.1); the two rows show it on the light
//! and dark grounds so the owner can eyeball each state. Auto-advances every
//! 1.4 s; click advances now. macOS only: GPUI is macOS-only in `jisuno-ui`.
//!
//! ```sh
//! cargo run -p jisuno-ui --example hud_gallery
//! ```

#[cfg(target_os = "macos")]
mod gallery {
    use std::time::Duration;

    use gpui::{
        div, prelude::*, px, rgb, size, App, Bounds, Context, Entity, MouseButton, TitlebarOptions,
        Window, WindowBounds, WindowOptions,
    };
    use jisuno_ui::hud_model::{all_samples, render_model};
    use jisuno_ui::hud_view::HudView;
    use jisuno_ui::tokens::{QUIET_DARK, QUIET_LIGHT};

    struct Gallery {
        index: usize,
        top: Entity<HudView>,
        bottom: Entity<HudView>,
    }

    impl Gallery {
        fn show(&mut self, index: usize, cx: &mut Context<Self>) {
            let samples = all_samples();
            self.index = index % samples.len().max(1);
            let state = samples[self.index].1.clone();
            for view in [&self.top, &self.bottom] {
                view.update(cx, |view, cx| {
                    view.set_state(state.clone(), cx);
                });
            }
            cx.notify();
        }

        fn advance(&mut self, cx: &mut Context<Self>) {
            let next = self.index + 1;
            self.show(next, cx);
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(1400))
                    .await;
                this.update(cx, |this, cx| this.advance(cx)).ok();
            })
            .detach();
        }
    }

    impl Render for Gallery {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let samples = all_samples();
            let (label, state) = &samples[self.index];
            let dismiss = render_model(state)
                .dismiss_after_ms
                .map(|ms| format!("{ms} ms"))
                .unwrap_or_else(|| "sticky".into());
            let click = cx.listener(|this: &mut Self, _, _, cx| {
                let next = this.index + 1;
                this.show(next, cx);
            });
            div()
                .flex()
                .flex_col()
                .size_full()
                .bg(rgb(QUIET_LIGHT.bg))
                .on_mouse_down(MouseButton::Left, click)
                .child(
                    div()
                        .px(px(16.0))
                        .py(px(10.0))
                        .text_size(px(12.0))
                        .font_family("SF Pro")
                        .text_color(rgb(QUIET_LIGHT.soft))
                        .child(format!(
                            "{}/{} · {} · {} · click advances",
                            self.index + 1,
                            samples.len(),
                            label,
                            dismiss
                        )),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(12.0))
                        .bg(rgb(QUIET_LIGHT.ground))
                        .py(px(16.0))
                        .child(self.top.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(12.0))
                        .bg(rgb(QUIET_DARK.ground))
                        .py(px(16.0))
                        .child(self.bottom.clone()),
                )
        }
    }

    pub fn run() {
        gpui_platform::application().run(|cx: &mut App| {
            if let Err(error) = jisuno_ui::hud_view::load_system_fonts(cx) {
                eprintln!("hud_gallery: {error:#}");
            }
            let bounds = Bounds::centered(None, size(px(680.0), px(560.0)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("HUD gallery".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|cx| {
                        let first = all_samples()[0].1.clone();
                        let top = cx.new(|_| HudView::new(first.clone()));
                        let bottom = cx.new(|_| HudView::new(first));
                        let mut gallery = Gallery {
                            index: 0,
                            top,
                            bottom,
                        };
                        gallery.advance(cx);
                        gallery
                    })
                },
            )
            .unwrap();
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
    eprintln!("hud_gallery needs macOS: GPUI is macOS-only in jisuno-ui.");
}
