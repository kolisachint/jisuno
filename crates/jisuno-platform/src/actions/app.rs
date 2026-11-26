//! `app.open {app, arg?}`: launch the app, opening `arg` in it.
//!
//! Resolution order: a dotted name goes through
//! `NSWorkspace URLForApplicationWithBundleIdentifier`; otherwise the three
//! Applications folders (`/Applications`, `~/Applications`,
//! `/System/Applications`) are scanned for `<app>.app` (case-insensitive).
//! Nothing found is `app not installed: <app>`. Launching uses `open -a`
//! semantics with the resolved path.

use std::path::Path;

use super::expand_tilde_live;

/// Where `app.open` looks for `<app>.app`, in order.
pub(crate) fn applications_dirs() -> Vec<String> {
    let mut dirs = vec![
        "/Applications".to_string(),
        "/System/Applications".to_string(),
    ];
    if let Ok(home) = std::env::var("HOME") {
        dirs.insert(1, format!("{home}/Applications"));
    }
    dirs
}

/// Find `<name>.app` under `dirs` (case-insensitive). Pure over the dir
/// list: unit-tested on all OSes with temp dirs.
pub(crate) fn find_app_bundle(name: &str, dirs: &[String]) -> Option<String> {
    let want = format!("{name}.app").to_lowercase();
    for dir in dirs {
        let entries = std::fs::read_dir(dir).ok()?;
        for entry in entries.flatten() {
            let file_name = entry.file_name().to_string_lossy().to_lowercase();
            if file_name == want {
                return Some(entry.path().to_string_lossy().into_owned());
            }
        }
    }
    None
}

/// How `arg` is opened: URLs go to the browser/handler, files open in the
/// app. Pure: unit-tested on all OSes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArgKind {
    Url,
    Path,
}

pub(crate) fn classify_arg(arg: &str) -> ArgKind {
    if arg.contains("://") {
        ArgKind::Url
    } else {
        ArgKind::Path
    }
}

/// Launch `app`, opening `arg` in it. `arg` is a file path (`~` expanded) or
/// a URL; empty args just launch/activate.
pub(crate) fn open_app(app: &str, arg: Option<&str>) -> Result<(), String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, arg);
        return Err("jisuno-platform: macOS only".to_string());
    }
    #[cfg(target_os = "macos")]
    {
        macos::open_app(app, arg)
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use objc2_foundation::NSString;

    use super::*;

    /// Resolve a display name to an app bundle path, or fail with the spec
    /// reason. Dotted names try the bundle-id registry first.
    fn resolve(app: &str) -> Result<String, String> {
        // NOTE: call from the main thread; AppKit may misbehave otherwise.
        let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
        if app.contains('.') {
            let id = NSString::from_str(app);
            if let Some(url) = workspace.URLForApplicationWithBundleIdentifier(&id) {
                return Ok(url.path().map(|p| p.to_string()).unwrap_or_else(|| {
                    url.absoluteString()
                        .map(|s| s.to_string())
                        .unwrap_or_default()
                }));
            }
        }
        if let Some(path) = find_app_bundle(app, &applications_dirs()) {
            return Ok(path);
        }
        Err(format!("app not installed: {app}"))
    }

    pub(super) fn open_app(app: &str, arg: Option<&str>) -> Result<(), String> {
        let resolved = resolve(app)?;
        let mut command = std::process::Command::new("open");
        // `-a` with the resolved path: identical semantics to `open -a <name>`,
        // but the lookup above already produced the exact `app not installed`
        // failure instead of `open`'s own error text.
        command.arg("-a").arg(&resolved);
        if let Some(arg) = arg.filter(|arg| !arg.is_empty()) {
            match classify_arg(arg) {
                ArgKind::Url => {
                    command.arg(arg);
                }
                ArgKind::Path => {
                    let expanded = expand_tilde_live(arg);
                    // `open` needs the target to exist for `-a` + file; pass
                    // directories and files through, and anything else as a
                    // plain argument so the app can interpret it.
                    if Path::new(&expanded).exists() {
                        command.arg(expanded);
                    } else {
                        command.arg(arg);
                    }
                }
            }
        }
        let status = command
            .status()
            .map_err(|error| format!("app not installed: {app} ({error:#})"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("app not installed: {app}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_urls_and_paths() {
        assert_eq!(classify_arg("http://localhost:4000"), ArgKind::Url);
        assert_eq!(classify_arg("https://example.com/x"), ArgKind::Url);
        assert_eq!(classify_arg("~/github/jisuno"), ArgKind::Path);
        assert_eq!(classify_arg("/tmp/note.md"), ArgKind::Path);
        assert_eq!(classify_arg("standup.md"), ArgKind::Path);
    }

    #[test]
    fn find_app_bundle_matches_case_insensitively() {
        let root = std::env::temp_dir().join(format!("jisuno-actions-test-{}", std::process::id()));
        let apps = root.join("Applications");
        std::fs::create_dir_all(apps.join("Zed.app")).expect("fixture Zed.app");
        std::fs::create_dir_all(apps.join("Safari.app")).expect("fixture Safari.app");
        let dirs = vec![apps.to_string_lossy().into_owned()];

        assert_eq!(
            find_app_bundle("Zed", &dirs),
            Some(apps.join("Zed.app").to_string_lossy().into_owned())
        );
        assert_eq!(
            find_app_bundle("zed", &dirs),
            Some(apps.join("Zed.app").to_string_lossy().into_owned())
        );
        assert_eq!(find_app_bundle("Missing", &dirs), None);
        assert_eq!(find_app_bundle("Zed", &[]), None);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn empty_dir_list_finds_nothing() {
        assert_eq!(find_app_bundle("TextEdit", &[]), None);
    }
}
