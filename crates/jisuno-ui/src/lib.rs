//! `jisuno-ui`: theme tokens and the HUD view for the Jisuno app.
//!
//! [`tokens`] and [`hud_model`] are pure and build everywhere, including
//! Linux CI. [`hud_view`] is the GPUI island view and is macOS-only, like the
//! rest of the app shell.
//!
//! Next step (not this foundation): the popover (menu bar menu), the Settings
//! window and the Welcome window.

pub mod hud_model;
pub mod tokens;

#[cfg(target_os = "macos")]
pub mod hud_view;

#[cfg(target_os = "macos")]
pub mod hud_window;
