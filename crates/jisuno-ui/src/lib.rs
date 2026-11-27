//! `jisuno-ui`: theme tokens and the GPUI views for the Jisuno app.
//!
//! The `*_model` modules and [`tokens`] are pure and build everywhere,
//! including Linux CI. The `*_view` modules are GPUI island views and are
//! macOS-only, like the rest of the app shell. Each view takes a plain
//! view-model struct and emits typed intents through a callback; the app
//! agent hosts them, performs the intents and feeds updated models back.

pub mod hud_model;
pub mod popover_model;
pub mod settings_model;
pub mod tokens;
pub mod welcome_model;

#[cfg(target_os = "macos")]
pub mod hud_view;
#[cfg(target_os = "macos")]
pub mod hud_window;
#[cfg(target_os = "macos")]
pub mod popover_view;
#[cfg(target_os = "macos")]
pub mod settings_view;
#[cfg(target_os = "macos")]
pub mod welcome_view;
