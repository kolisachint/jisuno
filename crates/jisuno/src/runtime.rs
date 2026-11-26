//! Pure voice-command runtime values: fixed slot lists plus host inventory.
//!
//! `build_runtime` combines running app names, installed app names and the
//! installed themes into [`runner::Runtime`][jisuno_logic::runner::Runtime].
//! `scan_installed_apps` lists `<name>` for every `<name>.app` under the
//! given Applications dirs. Both are pure over their inputs, so the matcher
//! bridge stays unit-testable on every OS (the macOS-only parts —
//! `NSWorkspace runningApplications`, the live Applications dirs — live in
//! `app::command`).

use jisuno_logic::runner::Runtime;

/// Installed themes (`{theme}`). The app ships exactly these two.
pub const THEMES: [&str; 2] = ["quiet-light", "quiet-dark"];

/// Build the matcher's host values from live inventory.
pub fn build_runtime(running: Vec<String>, installed: Vec<String>) -> Runtime {
    Runtime {
        windows: running,
        themes: THEMES.iter().map(|name| (*name).to_string()).collect(),
        apps: installed,
    }
}

/// Where `app.open` looks for `<app>.app`, in order.
pub fn applications_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = vec![std::path::PathBuf::from("/Applications")];
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(std::path::PathBuf::from(home).join("Applications"));
    }
    dirs.push(std::path::PathBuf::from("/System/Applications"));
    dirs
}

/// Names of installed apps: `<name>` for each `<name>.app` entry under
/// `dirs`. Sorted and de-duplicated for stable matching.
pub fn scan_installed_apps(dirs: &[std::path::PathBuf]) -> Vec<String> {
    let mut names: Vec<String> = dirs
        .iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flat_map(|entries| entries.flatten())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".app").map(str::to_string)
        })
        .filter(|name| !name.is_empty())
        .collect();
    names.sort();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_carries_windows_themes_and_apps() {
        let runtime = build_runtime(
            vec!["Safari".to_string()],
            vec!["TextEdit".to_string(), "Safari".to_string()],
        );
        assert_eq!(runtime.windows, vec!["Safari".to_string()]);
        assert_eq!(
            runtime.themes,
            vec!["quiet-light".to_string(), "quiet-dark".to_string()]
        );
        assert_eq!(
            runtime.apps,
            vec!["TextEdit".to_string(), "Safari".to_string()]
        );
    }

    #[test]
    fn scan_lists_app_bundles_sorted() {
        let tmp = tempfile::tempdir().expect("tempdir");
        for name in ["Zed.app", "TextEdit.app", "notes.txt", "Rex.app"] {
            std::fs::create_dir(tmp.path().join(name)).expect("mkdir");
        }
        let missing = tmp.path().join("gone");
        let dirs = vec![tmp.path().to_path_buf(), missing];
        let names = scan_installed_apps(&dirs);
        assert_eq!(names, vec!["Rex", "TextEdit", "Zed"]);
    }

    #[test]
    fn scan_skips_bare_app_suffix() {
        let tmp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(tmp.path().join(".app")).expect("mkdir");
        let dirs = vec![tmp.path().to_path_buf()];
        assert!(scan_installed_apps(&dirs).is_empty());
    }
}
