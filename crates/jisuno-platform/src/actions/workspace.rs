//! `workspace.open {workspace}` / `workspace.close {workspace}`.
//!
//! Open launches each window in turn: the app (via [`super::app`]), the
//! `open` target inside it, and `run` commands as new terminal windows. Each
//! window is then tiled to its `place`. Close presses each window's
//! `AXCloseButton` — apps are never quit.
//!
//! `run` commands: Jisuno cannot script terminals without Apple Events, so a
//! `.command` file (`#!/bin/zsh -l` + the command) is written under the temp
//! dir and opened with the window's `app`. Limits: Terminal.app opens a new
//! window and runs the file (honoring its own close-window prefs); other
//! terminals may ignore `-a` handling or open the file elsewhere —
//! `workspace.open` reports what the window lookup finds, not what the
//! terminal did. The files persist (`<tempdir>/jisuno-run-<id>.command`) so
//! re-running a workspace reuses them.

use jisuno_logic::voice_control::WindowDef;

/// Open every window of the workspace in turn.
pub(crate) fn open_workspace(defs: &[WindowDef]) -> Result<(), String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = defs;
        Err("jisuno-platform: macOS only".to_string())
    }
    #[cfg(target_os = "macos")]
    {
        macos::open_workspace(defs)
    }
}

/// Close every window of the workspace (never quits apps).
pub(crate) fn close_workspace(defs: &[WindowDef]) -> Result<(), String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = defs;
        Err("jisuno-platform: macOS only".to_string())
    }
    #[cfg(target_os = "macos")]
    {
        macos::close_workspace(defs)
    }
}

/// File name for a def's `run` command file. Pure: unit-tested on all OSes.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn command_file_name(id: &str) -> String {
    let safe: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("jisuno-run-{safe}.command")
}

/// Contents of the `.command` file: login shell, then the command.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn command_file_contents(run: &str) -> String {
    format!("#!/bin/zsh -l\n{run}\n")
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::c_void;
    use std::os::unix::fs::PermissionsExt;

    use super::super::ax::macos as axm;
    use super::super::{resolve_window, ResolvedWindow};
    use super::*;

    /// Wait for the window (title match when given), then tile it.
    fn place_def(
        def: &WindowDef,
        workspaces: &std::collections::BTreeMap<String, Vec<WindowDef>>,
    ) -> Result<(), String> {
        let resolved = resolve_window(&def.id, workspaces);
        super::super::wait::wait_for_window(&resolved)?;
        super::super::window::tile_window(&resolved, def.place)
    }

    /// Write (or reuse) the `.command` file for `run`, returning its path.
    fn write_command_file(id: &str, run: &str) -> Result<std::path::PathBuf, String> {
        let path = std::env::temp_dir().join(command_file_name(id));
        std::fs::write(&path, command_file_contents(run))
            .map_err(|error| format!("no window \"{id}\" ({error:#})"))?;
        let mut permissions = std::fs::metadata(&path)
            .map_err(|error| format!("no window \"{id}\" ({error:#})"))?
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions)
            .map_err(|error| format!("no window \"{id}\" ({error:#})"))?;
        Ok(path)
    }

    pub(super) fn open_workspace(defs: &[WindowDef]) -> Result<(), String> {
        // A one-entry map so `resolve_window` matches these defs by id.
        let workspaces =
            std::collections::BTreeMap::from([("workspace".to_string(), defs.to_vec())]);
        for def in defs {
            // Launch first: `open` targets and `run` files need the app.
            super::super::app::open_app(&def.app, None)?;
            if let Some(open) = def.open.as_deref().filter(|open| !open.is_empty()) {
                let arg = super::super::expand_tilde_live(open);
                super::super::app::open_app(&def.app, Some(&arg))?;
            }
            if let Some(run) = def.run.as_deref().filter(|run| !run.is_empty()) {
                let file = write_command_file(&def.id, run)?;
                super::super::app::open_app(&def.app, Some(&file.to_string_lossy()))?;
            }
            place_def(def, &workspaces)?;
        }
        Ok(())
    }

    pub(super) fn close_workspace(defs: &[WindowDef]) -> Result<(), String> {
        for def in defs {
            let resolved = ResolvedWindow {
                label: def.id.clone(),
                app: def.app.clone(),
                title: def.title.clone(),
            };
            let (windows, element) = axm::find_window_element(&resolved)
                .map_err(|_| format!("no window \"{}\"", def.id))?;
            // Keep the list alive across the button lookup and press.
            let _windows = windows;
            let mut held = Vec::new();
            // Window chrome buttons carry no title; the lookup matches on
            // `AXDescription` where present, else the attribute fallback
            // below handles it.
            let buttons = axm::lookup_elements(element, "AXButton", "Close", &mut held);
            // Fall back to the window's own close button attribute.
            let target: *const c_void = match buttons {
                Ok(found) if !found.is_empty() => found[0],
                _ => match axm::copy_attr(element, "AXCloseButton") {
                    Ok(Some(button)) => {
                        let ptr = button.get();
                        held.push(button);
                        ptr
                    }
                    _ => return Err(format!("no window \"{}\"", def.id)),
                },
            };
            if axm::perform(target, "AXPress") != axm::AX_OK {
                return Err(format!("no window \"{}\"", def.id));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_file_names_are_safe() {
        assert_eq!(command_file_name("terminal"), "jisuno-run-terminal.command");
        assert_eq!(
            command_file_name("my term/2"),
            "jisuno-run-my_term_2.command"
        );
    }

    #[test]
    fn command_file_runs_login_shell() {
        assert_eq!(command_file_contents("hoocode"), "#!/bin/zsh -l\nhoocode\n");
    }
}
