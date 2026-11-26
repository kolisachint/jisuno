//! Voice-command actions: the macOS side of `jisuno_logic::runner::Actions`.
//!
//! [`MacActions`] runs one [`ResolvedStep`][jisuno_logic::runner::ResolvedStep]
//! at a time. Pure pieces (place math in [`geom`], path handling, key mapping)
//! are framework-free and unit-tested on all OSes; everything that touches
//! AppKit, AX, or CGEvent lives behind `#[cfg(target_os = "macos")]` and
//! returns `Err("jisuno-platform: macOS only")` elsewhere, like the rest of
//! the crate.
//!
//! Failure reasons are the exact strings from `docs/design/voice-control.md`
//! §4 (`app not installed: Foo`, `no window "terminal"`,
//! `tile failed: no permission`, `AX element not found: AXButton "Send"`,
//! `no focused app`, `shell exited 1`, `unknown theme "quiet-blue"`,
//! `model not installed`, `timed out after 5 s`).
//!
//! Call from the main thread: AppKit (`NSWorkspace`, `NSScreen`) misbehaves
//! otherwise.

pub mod app;
pub mod ax;
pub mod delegated;
pub mod geom;
pub mod keys_send;
pub mod shell;
pub mod wait;
pub mod window;
pub mod workspace;

use std::collections::BTreeMap;

use jisuno_logic::runner::{Actions, ResolvedStep};
use jisuno_logic::voice_control::WindowDef;

/// App-owned actions. The app supplies this (as a trait object inside
/// [`MacActions`]); theme/model/menu/settings steps delegate to it, and its
/// `Err` strings become the step failure reasons verbatim (e.g. the app
/// returns `unknown theme "quiet-blue"` from `set_theme`, `model not
/// installed` from `warm_model`).
pub trait AppHooks {
    /// Switch the theme; unknown names fail with `unknown theme "<name>"`.
    fn set_theme(&mut self, name: &str) -> Result<(), String>;
    /// Start loading the model; missing files fail with `model not installed`.
    fn warm_model(&mut self) -> Result<(), String>;
    /// Unload the model from memory.
    fn unload_model(&mut self) -> Result<(), String>;
    /// Open the menu bar popover.
    fn open_menu(&mut self) -> Result<(), String>;
    /// Open the Settings window.
    fn open_settings(&mut self) -> Result<(), String>;
}

/// The macOS [`Actions`] implementation. Holds the app's [`AppHooks`] for the
/// app-owned steps; everything else runs here.
pub struct MacActions {
    hooks: Box<dyn AppHooks>,
}

impl MacActions {
    /// Build from the app's hooks.
    pub fn new(hooks: impl AppHooks + 'static) -> Self {
        Self {
            hooks: Box::new(hooks),
        }
    }
}

impl Actions for MacActions {
    fn run(
        &mut self,
        step: &ResolvedStep,
        workspaces: &BTreeMap<String, Vec<WindowDef>>,
    ) -> Result<(), String> {
        match step {
            ResolvedStep::JisunoMenu => self.hooks.open_menu(),
            ResolvedStep::JisunoSettings => self.hooks.open_settings(),
            ResolvedStep::AppOpen { app, arg } => app::open_app(app, arg.as_deref()),
            ResolvedStep::WindowFocus { window } => {
                let resolved = resolve_window(window, workspaces);
                window::focus_window(&resolved)
            }
            ResolvedStep::WindowTile { window, place } => {
                let resolved = resolve_window(window, workspaces);
                window::tile_window(&resolved, *place)
            }
            ResolvedStep::WorkspaceOpen { workspace } => {
                let defs = workspaces
                    .get(workspace)
                    .ok_or_else(|| format!("no window \"{workspace}\""))?;
                workspace::open_workspace(defs)
            }
            ResolvedStep::WorkspaceClose { workspace } => {
                let defs = workspaces
                    .get(workspace)
                    .ok_or_else(|| format!("no window \"{workspace}\""))?;
                workspace::close_workspace(defs)
            }
            ResolvedStep::AxPress { app, role, name } => ax::press(app, role, name),
            ResolvedStep::AxSetValue {
                app,
                role,
                name,
                value,
            } => ax::set_value(app, role, name, value),
            ResolvedStep::KeysSend { keys } => keys_send::send_keys(keys),
            ResolvedStep::ShellRun { cmd, cwd } => shell::run_command(cmd, cwd.as_deref()),
            ResolvedStep::ThemeSet { theme } => self.hooks.set_theme(theme),
            ResolvedStep::ModelWarm => self.hooks.warm_model(),
            ResolvedStep::ModelUnload => self.hooks.unload_model(),
            ResolvedStep::Wait { window, ms } => match (window, ms) {
                (Some(window), _) => {
                    let resolved = resolve_window(window, workspaces);
                    wait::wait_for_window(&resolved)
                }
                (None, Some(ms)) => {
                    wait::wait_ms(*ms);
                    Ok(())
                }
                (None, None) => Ok(()),
            },
        }
    }
}

/// A window reference with the workspace id resolved: `app` names the app,
/// `title` (when present) substring-matches the window title, `label` is the
/// original reference for failure reasons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedWindow {
    /// Original `window` argument, used in `no window "<window>"`.
    pub(crate) label: String,
    /// App name (workspace def `app`, or the literal reference).
    pub(crate) app: String,
    /// Title filter from the workspace def, if any.
    pub(crate) title: Option<String>,
}

/// Resolve a `window` step argument: a workspace window id (matched against
/// every workspace's defs) or, failing that, a running app name.
pub(crate) fn resolve_window(
    window: &str,
    workspaces: &BTreeMap<String, Vec<WindowDef>>,
) -> ResolvedWindow {
    for defs in workspaces.values() {
        if let Some(def) = defs.iter().find(|def| def.id == window) {
            return ResolvedWindow {
                label: window.to_string(),
                app: def.app.clone(),
                title: def.title.clone(),
            };
        }
    }
    ResolvedWindow {
        label: window.to_string(),
        app: window.to_string(),
        title: None,
    }
}

/// Expand a leading `~` or `~/...` against `home`. Pure: unit-tested on all
/// OSes; live callers pass `HOME`.
pub(crate) fn expand_tilde(path: &str, home: &str) -> String {
    if path == "~" {
        return home.to_string();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if home.ends_with('/') {
            return format!("{home}{rest}");
        }
        return format!("{home}/{rest}");
    }
    path.to_string()
}

/// Live `~` expansion against `$HOME` (empty when unset).
pub(crate) fn expand_tilde_live(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    expand_tilde(path, &home)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_prefers_workspace_id() {
        let workspaces = BTreeMap::from([(
            "work".to_string(),
            vec![WindowDef {
                id: "terminal".to_string(),
                app: "Rex".to_string(),
                title: Some("hoocode".to_string()),
                open: None,
                run: Some("hoocode".to_string()),
                place: jisuno_logic::voice_control::Place::LeftHalf,
            }],
        )]);
        assert_eq!(
            resolve_window("terminal", &workspaces),
            ResolvedWindow {
                label: "terminal".to_string(),
                app: "Rex".to_string(),
                title: Some("hoocode".to_string()),
            }
        );
        // Unknown ids fall through to app names.
        assert_eq!(
            resolve_window("Safari", &workspaces),
            ResolvedWindow {
                label: "Safari".to_string(),
                app: "Safari".to_string(),
                title: None,
            }
        );
    }

    #[test]
    fn tilde_expands_only_leading() {
        assert_eq!(expand_tilde("~/a/b", "/Users/x"), "/Users/x/a/b");
        assert_eq!(expand_tilde("~", "/Users/x"), "/Users/x");
        assert_eq!(expand_tilde("/a/~/b", "/Users/x"), "/a/~/b");
        assert_eq!(expand_tilde("rel/path", "/Users/x"), "rel/path");
        assert_eq!(expand_tilde("~/a", "/Users/x/"), "/Users/x/a");
    }

    struct StubHooks {
        theme_error: Option<String>,
    }

    impl AppHooks for StubHooks {
        fn set_theme(&mut self, name: &str) -> Result<(), String> {
            match &self.theme_error {
                Some(error) => Err(error.clone()),
                None => {
                    assert!(!name.is_empty());
                    Ok(())
                }
            }
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

    #[test]
    fn delegated_steps_pass_reasons_through() {
        let mut actions = MacActions::new(StubHooks { theme_error: None });
        let workspaces = BTreeMap::new();
        assert!(
            actions
                .run(&ResolvedStep::ThemeSet { theme: "x".into() }, &workspaces)
                .is_ok()
        );
        assert_eq!(
            actions.run(&ResolvedStep::ModelWarm, &workspaces),
            Err("model not installed".to_string())
        );
        assert!(actions.run(&ResolvedStep::ModelUnload, &workspaces).is_ok());
        assert!(actions.run(&ResolvedStep::JisunoMenu, &workspaces).is_ok());
        assert!(
            actions
                .run(&ResolvedStep::JisunoSettings, &workspaces)
                .is_ok()
        );

        let mut failing = MacActions::new(StubHooks {
            theme_error: Some("unknown theme \"quiet-blue\"".to_string()),
        });
        assert_eq!(
            failing.run(
                &ResolvedStep::ThemeSet {
                    theme: "quiet-blue".into()
                },
                &workspaces
            ),
            Err("unknown theme \"quiet-blue\"".to_string())
        );
    }

    #[test]
    fn unknown_workspace_names_the_workspace() {
        let mut actions = MacActions::new(StubHooks { theme_error: None });
        assert_eq!(
            actions.run(
                &ResolvedStep::WorkspaceOpen {
                    workspace: "wrk".into()
                },
                &BTreeMap::new(),
            ),
            Err("no window \"wrk\"".to_string())
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn stubs_on_other_oses() {
        let mut actions = MacActions::new(StubHooks { theme_error: None });
        let resolved = resolve_window("terminal", &BTreeMap::new());
        assert_eq!(
            window::focus_window(&resolved),
            Err("jisuno-platform: macOS only".to_string())
        );
        assert_eq!(
            app::open_app("TextEdit", None),
            Err("jisuno-platform: macOS only".to_string())
        );
    }
}
