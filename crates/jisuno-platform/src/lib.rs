//! macOS glue for Jisuno: event tap, text insertion, app identity, permissions.
//!
//! The crate compiles on every OS (Linux CI included). Everything that needs
//! macOS frameworks lives behind `#[cfg(target_os = "macos")]`; on other
//! OSes the same public API exists but the failable functions return
//! `Err(anyhow!("jisuno-platform: macOS only"))`.
//!
//! Pure helpers (flag-bit decoding, chord translation, UTF-16 chunking, the
//! settings-pane URLs) are framework-free and unit-tested on all OSes.

pub mod actions;
pub mod frontmost;
pub mod hud_placement;
pub mod insert;
pub mod keys;
pub mod login_item;
pub mod permissions;
pub mod secure_input;
pub mod status_item;
