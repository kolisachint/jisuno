//! Config file watcher: poll mtime+size on a background thread, no new deps.
//!
//! [`FileWatch`] snapshots one file (mtime + size; `None` when missing) and
//! reports `true` once per change. [`spawn_watcher`] polls a path list every
//! `interval` and sends each changed path; the tick thread drains it and
//! re-parses. Parsing, last-good and HUD stay on the tick (they need `App`).

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

/// Snapshot of one watched file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileWatch {
    path: PathBuf,
    mtime: Option<SystemTime>,
    size: Option<u64>,
}

impl FileWatch {
    /// Record the current snapshot. The first [`changed`][Self::changed]
    /// after this fires only on a later change.
    pub fn new(path: PathBuf) -> Self {
        let (mtime, size) = snapshot(&path);
        Self { path, mtime, size }
    }

    /// True once when mtime or size differs from the snapshot (including
    /// create/delete), then re-arms on the new snapshot.
    pub fn changed(&mut self) -> bool {
        let (mtime, size) = snapshot(&self.path);
        if (mtime, size) != (self.mtime, self.size) {
            self.mtime = mtime;
            self.size = size;
            true
        } else {
            false
        }
    }
}

fn snapshot(path: &std::path::Path) -> (Option<SystemTime>, Option<u64>) {
    match std::fs::metadata(path) {
        Ok(meta) => (meta.modified().ok(), Some(meta.len())),
        Err(_) => (None, None),
    }
}

/// Poll `paths` every `interval` on a background thread; each detected change
/// sends the path. The initial snapshot is taken at spawn, so pre-existing
/// content never fires.
pub fn spawn_watcher(paths: Vec<PathBuf>, interval: Duration) -> mpsc::Receiver<PathBuf> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut watches: Vec<FileWatch> = paths.into_iter().map(FileWatch::new).collect();
        loop {
            std::thread::sleep(interval);
            let mut changed = Vec::new();
            for watch in &mut watches {
                if watch.changed() {
                    changed.push(watch.path.clone());
                }
            }
            let mut gone = false;
            for path in changed {
                if tx.send(path).is_err() {
                    gone = true;
                    break;
                }
            }
            if gone {
                break;
            }
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_file_never_fires() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("keybindings.json");
        std::fs::write(&path, "{\"bindings\": []}").expect("write");
        let mut watch = FileWatch::new(path);
        assert!(!watch.changed());
        assert!(!watch.changed());
    }

    #[test]
    fn content_change_fires_once() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, "{}").expect("write");
        let mut watch = FileWatch::new(path.clone());
        std::fs::write(&path, "{\"sounds\": false}").expect("rewrite");
        assert!(watch.changed());
        assert!(!watch.changed());
    }

    #[test]
    fn delete_and_recreate_fire() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("theme.json");
        std::fs::write(&path, "{}").expect("write");
        let mut watch = FileWatch::new(path.clone());
        std::fs::remove_file(&path).expect("remove");
        assert!(watch.changed());
        assert!(!watch.changed());
        std::fs::write(&path, "{}").expect("recreate");
        assert!(watch.changed());
        assert!(!watch.changed());
    }

    #[test]
    fn missing_file_stays_quiet_until_created() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("voice-control-config.json");
        let mut watch = FileWatch::new(path.clone());
        assert!(!watch.changed());
        std::fs::write(&path, "{}").expect("create");
        assert!(watch.changed());
    }
}
