//! Action probe: runs a few safe live actions and reports each step.
//!
//! Safe by design: opens TextEdit, waits for its window, focuses it, tiles
//! it right-half, then restores full frame. No `shell.run`, no `keys.send`
//! (nothing is typed or clicked), no workspace changes.
//!
//! Usage: `cargo run -p jisuno-platform --example action_probe`.
//! Needs Accessibility for tile; without it the tile step reports
//! `tile failed: no permission` and the probe keeps going.

use std::collections::BTreeMap;
use std::time::Duration;

use jisuno_logic::runner::{Actions, ResolvedStep};
use jisuno_logic::voice_control::Place;
use jisuno_platform::actions::MacActions;
use jisuno_platform::{frontmost, permissions};

struct ProbeHooks;

impl jisuno_platform::actions::AppHooks for ProbeHooks {
    fn set_theme(&mut self, name: &str) -> Result<(), String> {
        Err(format!("unknown theme \"{name}\""))
    }
    fn warm_model(&mut self) -> Result<(), String> {
        Err("model not installed".to_string())
    }
    fn unload_model(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn open_menu(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn open_settings(&mut self) -> Result<(), String> {
        Ok(())
    }
}

fn main() -> anyhow::Result<()> {
    println!("accessibility:    {:?}", permissions::accessibility());
    if let Some(app) = frontmost::frontmost_app() {
        println!("frontmost now:    {app:?}");
    }

    let mut actions = MacActions::new(ProbeHooks);
    let workspaces = BTreeMap::new();
    let steps = [
        ResolvedStep::AppOpen {
            app: "TextEdit".to_string(),
            arg: None,
        },
        ResolvedStep::Wait {
            window: Some("TextEdit".to_string()),
            ms: None,
        },
        ResolvedStep::WindowFocus {
            window: "TextEdit".to_string(),
        },
        ResolvedStep::WindowTile {
            window: "TextEdit".to_string(),
            place: Place::RightHalf,
        },
    ];
    for step in &steps {
        match actions.run(step, &workspaces) {
            Ok(()) => println!("ok:   {}", step.summary()),
            Err(reason) => println!("fail: {} · {reason}", step.action()),
        }
    }

    // Delegation demo: the app owns themes, so the probe's hook rejects it.
    match actions.run(
        &ResolvedStep::ThemeSet {
            theme: "quiet-blue".to_string(),
        },
        &workspaces,
    ) {
        Ok(()) => println!("ok:   theme.set quiet-blue"),
        Err(reason) => println!("fail: theme.set · {reason}"),
    }

    println!("holding right-half 2 s, then restoring full frame ...");
    std::thread::sleep(Duration::from_secs(2));
    match actions.run(
        &ResolvedStep::WindowTile {
            window: "TextEdit".to_string(),
            place: Place::Full,
        },
        &workspaces,
    ) {
        Ok(()) => println!("ok:   window.tile TextEdit full"),
        Err(reason) => println!("fail: window.tile · {reason}"),
    }
    if let Some(app) = frontmost::frontmost_app() {
        println!("frontmost now:    {app:?}");
    }
    Ok(())
}
