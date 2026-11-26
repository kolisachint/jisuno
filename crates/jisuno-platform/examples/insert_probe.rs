//! Insert probe: waits 3 s (click the target app), then inserts the given
//! text into the frontmost app via Paste with clipboard restore.
//!
//! Usage: `cargo run -p jisuno-platform --example insert_probe -- "hello" [paste|type]`.

use std::time::Duration;

use jisuno_platform::frontmost;
use jisuno_platform::insert::{insert_text, InsertMode};

fn main() -> anyhow::Result<()> {
    let text: String = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "jisuno insert probe".into());
    let mode = match std::env::args().nth(2).as_deref() {
        Some("type") => InsertMode::Type,
        _ => InsertMode::Paste,
    };
    println!("mode: {mode:?}");
    let app = frontmost::frontmost_app();
    println!("frontmost now: {app:?}");
    println!("inserting {text:?} in 3 s — focus the target app ...");
    std::thread::sleep(Duration::from_secs(3));

    let app = frontmost::frontmost_app();
    println!("target: {app:?}");
    let pid = app.map(|app| app.pid);
    match insert_text(&text, pid, mode, true) {
        Ok(()) => println!("inserted via {mode:?}"),
        Err(e) => println!("insert failed: {e:#}"),
    }
    // Clipboard restore runs ~150 ms later on a background thread: outlive it.
    std::thread::sleep(Duration::from_millis(500));
    Ok(())
}
