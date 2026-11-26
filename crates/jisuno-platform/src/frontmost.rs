//! Frontmost-app identity: who had focus at key-down, so inserts and
//! per-app `when` clauses target the right app (`jisuno.md` §10 round 4.6).

/// The app in front. `bundle_id` is `None` for processes without one
/// (e.g. some menu-bar extras); the pid is always present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub bundle_id: Option<String>,
    pub name: String,
    pub pid: i32,
}

/// Names of running apps (`localizedName`), for the `{window}` slot: the
/// workspace window ids from the config plus these names form the value
/// list (`voice-control.md` §5). Empty when it cannot be determined.
///
/// NOTE: call from the main thread; AppKit may misbehave otherwise.
pub fn running_app_names() -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSWorkspace;

        let workspace = NSWorkspace::sharedWorkspace();
        workspace
            .runningApplications()
            .iter()
            .filter_map(|app| {
                app.localizedName()
                    .map(|name| name.to_string())
                    .filter(|name| !name.is_empty())
            })
            .collect()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

/// The currently frontmost app, or `None` when it cannot be determined
/// (no UI session, or non-macOS).
pub fn frontmost_app() -> Option<AppInfo> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSWorkspace;

        // NOTE: call from the main thread; AppKit may misbehave otherwise.
        let workspace = NSWorkspace::sharedWorkspace();
        let app = workspace.frontmostApplication()?;
        Some(AppInfo {
            bundle_id: app.bundleIdentifier().map(|id| id.to_string()),
            name: app
                .localizedName()
                .map(|name| name.to_string())
                .unwrap_or_else(|| format!("pid {}", app.processIdentifier())),
            pid: app.processIdentifier(),
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}
