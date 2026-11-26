//! Live event-tap probe: prints permission states, then every [`KeyEvent`]
//! with the hold machine's answer.
//!
//! Needs Input Monitoring (and Accessibility) for the tap to install; without
//! them `KeyTap::start` fails fast and the probe reports it. Hold Right ⌥ or
//! Right ⌘, press ⌃⌥Space / ⌃⌥W / Esc / Return, watch the decisions.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jisuno_logic::hold::{CapturedKey, HoldKind, HoldMachine, Input};
use jisuno_logic::keybindings::{defaults, Command, Key, Keybindings};
use jisuno_platform::keys::{Captured, KeyEvent, KeyTap, RightKey, ShouldCapture, TapConfig};
use jisuno_platform::{frontmost, permissions, secure_input};

fn hold_kind(bindings: &Keybindings, key: RightKey) -> Option<HoldKind> {
    let wanted = Key::Hold(key.into());
    bindings
        .bindings
        .iter()
        .find(|b| b.hold && b.key == wanted)
        .and_then(|b| match b.command {
            Command::Dictate => Some(HoldKind::Dictate),
            Command::VoiceCommand => Some(HoldKind::Command),
            _ => None,
        })
}

fn main() -> anyhow::Result<()> {
    println!("microphone:       {:?}", permissions::microphone());
    println!("accessibility:    {:?}", permissions::accessibility());
    println!("input_monitoring: {:?}", permissions::input_monitoring());
    println!(
        "secure input:     {}",
        secure_input::is_secure_input_enabled()
    );
    if let Some(holder) = secure_input::secure_input_holder() {
        println!("secure holder:    {holder:?}");
    }
    if let Some(app) = frontmost::frontmost_app() {
        println!("frontmost:        {app:?}");
    }

    let bindings: Keybindings = defaults();
    let holds: Vec<RightKey> = bindings
        .bindings
        .iter()
        .filter(|b| b.hold)
        .filter_map(|b| match b.key {
            Key::Hold(side) => Some(RightKey::from(side)),
            Key::Chord { .. } => None,
        })
        .collect();
    let chords: Vec<Key> = bindings
        .bindings
        .iter()
        .filter(|b| !b.hold && b.when.is_none())
        .filter_map(|b| match &b.key {
            Key::Chord { .. } => Some(b.key.clone()),
            Key::Hold(_) => None,
        })
        .collect();
    println!("holds: {holds:?}");
    println!(
        "chords: {:?}",
        chords.iter().map(Key::file).collect::<Vec<_>>()
    );

    let machine = Arc::new(Mutex::new(HoldMachine::default()));
    let machine_w = Arc::clone(&machine);
    let should_capture: ShouldCapture = Arc::new(move |captured: Captured| {
        let machine = machine_w.lock().unwrap_or_else(|e| e.into_inner());
        match captured {
            Captured::Escape => machine.captures(CapturedKey::Escape),
            Captured::Return => machine.captures(CapturedKey::Return),
            Captured::Chord(_) => true,
        }
    });

    let start = Instant::now();
    let now_ms = || start.elapsed().as_millis() as u64;
    let (tap, rx) = KeyTap::start(TapConfig { holds, chords }, should_capture)?;
    println!("tap running; Ctrl-C to quit");

    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(event) => {
                let mut machine = machine.lock().unwrap_or_else(|e| e.into_inner());
                println!("[{:6}ms] {event:?}", now_ms());
                let input = match &event {
                    KeyEvent::HoldDown(key) => {
                        if let Some(app) = frontmost::frontmost_app() {
                            println!("  frontmost on hold: {app:?}");
                        }
                        hold_kind(&bindings, *key).map(Input::HoldDown)
                    }
                    KeyEvent::HoldUp(key) => hold_kind(&bindings, *key).map(Input::HoldUp),
                    KeyEvent::OtherKeyDown => Some(Input::OtherKeyDown),
                    KeyEvent::Chord(key) => match bindings.lookup(key, None) {
                        Some(Command::HandsFreeToggle) => Some(Input::HandsFreeToggle),
                        Some(Command::Warm) => {
                            println!("  warm key (no machine input)");
                            None
                        }
                        Some(command) => {
                            println!("  chord for {command:?} (no machine input)");
                            None
                        }
                        None => None,
                    },
                    KeyEvent::Escape => Some(Input::Cancel),
                    KeyEvent::Return => Some(Input::Confirm),
                    KeyEvent::TapReenabled => {
                        println!("  tap re-enabled: keys may have been missed");
                        None
                    }
                };
                if let Some(input) = input {
                    for effect in machine.handle(input, now_ms()) {
                        println!("  -> {effect:?}");
                    }
                }
                for effect in machine.tick(now_ms()) {
                    println!("  -> {effect:?} (tick)");
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let mut machine = machine.lock().unwrap_or_else(|e| e.into_inner());
                for effect in machine.tick(now_ms()) {
                    println!("  -> {effect:?} (tick)");
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    tap.shutdown()
}
