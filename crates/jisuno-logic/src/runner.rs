//! Voice-command step runner: config + host values to executable plans.
//!
//! The [`Commands`] bridge owns everything the [`matcher`][crate::matcher]
//! needs (fixed slot lists plus declared command slots) and turns an
//! utterance into a [`Resolution`]. A [`Plan`] holds [`ResolvedStep`]s with
//! every `{slot}` filled from the match; the macOS host runs them through the
//! [`Actions`] trait via [`execute`]. `workspace.open`/`close` steps keep
//! their workspace name and expand into [`WindowDef`][crate::voice_control::WindowDef]s
//! at run time, inside the host's `Actions` implementation.
//!
//! Slot sources (`docs/design/voice-control.md` §5): `{window}` is the
//! workspace window ids from [`Config`][crate::voice_control::Config] plus the
//! running app names the host supplies in [`Runtime`]; `{place}` is the 10
//! place values; `{workspace}` is the config workspace keys; `{theme}` and
//! `{app}` come from [`Runtime`]; `{name}` and declared `"free"` slots are
//! free text; other declared slots use the command's value list.

use std::collections::BTreeMap;

use crate::matcher::{self, Candidate, SlotValues};
use crate::voice_control::{Config, Place, PlaceOrSlot, SlotDecl, Step, WindowDef};

/// Host-supplied values for the built-in fixed slots that the config cannot
/// know: running app names (`{window}`), installed themes (`{theme}`), and
/// installed apps (`{app}`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Runtime {
    /// Names of running apps; combined with the workspace window ids from the
    /// config to form the `{window}` value list.
    pub windows: Vec<String>,
    /// Installed theme names (`{theme}`).
    pub themes: Vec<String>,
    /// Installed app names (`{app}`).
    pub apps: Vec<String>,
}

/// One stored command: the owned data the matcher borrows.
#[derive(Debug, Clone, PartialEq)]
struct Entry {
    id: String,
    phrases: Vec<String>,
    when_apps: Option<Vec<String>>,
    confirm: bool,
    steps: Vec<Step>,
}

/// Matcher input built from a [`Config`] plus the host [`Runtime`].
///
/// Owns every string the matcher borrows (command ids, phrases, `when` lists,
/// and all slot value lists) so [`resolve`][Commands::resolve] can build
/// candidates without further allocation of config data.
#[derive(Debug, Clone)]
pub struct Commands {
    entries: Vec<Entry>,
    window_values: Vec<String>,
    place_values: Vec<String>,
    workspace_values: Vec<String>,
    themes: Vec<String>,
    apps: Vec<String>,
    declared: BTreeMap<(String, String), SlotDecl>,
}

impl Commands {
    /// Build the matcher bridge from a validated config and host values.
    pub fn new(config: &Config, runtime: &Runtime) -> Self {
        let mut window_values: Vec<String> = config
            .workspaces
            .values()
            .flat_map(|windows| windows.iter().map(|window| window.id.clone()))
            .collect();
        for app in &runtime.windows {
            if !window_values.contains(app) {
                window_values.push(app.clone());
            }
        }
        let place_values: Vec<String> = [
            Place::LeftHalf,
            Place::RightHalf,
            Place::TopHalf,
            Place::BottomHalf,
            Place::LeftTop,
            Place::LeftBottom,
            Place::RightTop,
            Place::RightBottom,
            Place::Full,
            Place::Center,
        ]
        .iter()
        .map(|place| place.as_str().to_string())
        .collect();
        let workspace_values: Vec<String> = config.workspaces.keys().cloned().collect();

        let mut declared = BTreeMap::new();
        let entries: Vec<Entry> = config
            .commands
            .iter()
            .map(|command| {
                for (slot, decl) in &command.slots {
                    declared.insert((command.id.clone(), slot.clone()), decl.clone());
                }
                Entry {
                    id: command.id.clone(),
                    phrases: command.say.clone(),
                    when_apps: command.when.as_ref().map(|when| when.app.clone()),
                    confirm: command.confirm,
                    steps: command.steps.clone(),
                }
            })
            .collect();

        Self {
            entries,
            window_values,
            place_values,
            workspace_values,
            themes: runtime.themes.clone(),
            apps: runtime.apps.clone(),
            declared,
        }
    }

    /// Match `utterance` against every command (frontmost-app `when` gates
    /// apply inside the matcher) and build the plan for an accepted match.
    ///
    /// A match on a command with `confirm: true` yields [`Resolution::Confirm`]
    /// instead of [`Resolution::Run`]. A fill failure (missing slot value or a
    /// non-place value for `place`) is treated as [`Resolution::NoMatch`]:
    /// with a validated config it cannot happen.
    pub fn resolve(&self, utterance: &str, frontmost_app: Option<&str>) -> Resolution {
        let candidates: Vec<Candidate<'_>> = self
            .entries
            .iter()
            .flat_map(|entry| {
                entry.phrases.iter().map(|phrase| Candidate {
                    command_id: entry.id.as_str(),
                    phrase: phrase.as_str(),
                    when_apps: entry.when_apps.as_deref(),
                })
            })
            .collect();
        match matcher::match_utterance(utterance, frontmost_app, &candidates, self) {
            matcher::Outcome::Run(matched) => {
                let Some(entry) = self.entries.iter().find(|e| e.id == matched.command_id) else {
                    return Resolution::NoMatch;
                };
                match fill_plan(&entry.id, &matched.phrase, &entry.steps, &matched.slots) {
                    Ok(plan) if entry.confirm => Resolution::Confirm(plan),
                    Ok(plan) => Resolution::Run(plan),
                    Err(_) => Resolution::NoMatch,
                }
            }
            matcher::Outcome::DidYouMean { phrase, .. } => {
                Resolution::DidYouMean(format!("Did you mean \"{phrase}\"?"))
            }
            matcher::Outcome::NoMatch => Resolution::NoMatch,
        }
    }
}

impl matcher::SlotSource for Commands {
    fn values(&self, command_id: &str, slot: &str) -> Option<SlotValues<'_>> {
        match slot {
            "window" => Some(SlotValues::List(&self.window_values)),
            "place" => Some(SlotValues::List(&self.place_values)),
            "workspace" => Some(SlotValues::List(&self.workspace_values)),
            "theme" => Some(SlotValues::List(&self.themes)),
            "app" => Some(SlotValues::List(&self.apps)),
            "name" => Some(SlotValues::Free),
            _ => match self
                .declared
                .get(&(command_id.to_string(), slot.to_string()))
            {
                Some(SlotDecl::List(values)) => Some(SlotValues::List(values)),
                Some(SlotDecl::Free) => Some(SlotValues::Free),
                None => None,
            },
        }
    }
}

/// What to do with an utterance: run now, confirm first, suggest, or nothing.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    /// Run the plan's steps in order.
    Run(Plan),
    /// The command needs confirmation; show [`Plan::confirm_prompt`] first.
    Confirm(Plan),
    /// HUD copy, exactly `Did you mean "<phrase>"?`.
    DidYouMean(String),
    /// No eligible command; nothing runs.
    NoMatch,
}

/// A matched command with every step's `{slot}` refs filled.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Id of the matched command.
    pub command_id: String,
    /// The matched phrase rendered with slots filled (e.g. `"clean build"`).
    pub label: String,
    /// Steps in run order.
    pub steps: Vec<ResolvedStep>,
}

impl Plan {
    /// HUD confirm copy: `Run "<label>"? <summary> · Return to run · Esc to
    /// cancel`. The summary is the shell command for a single `shell.run`
    /// step, otherwise the step summaries joined with `", "`.
    pub fn confirm_prompt(&self) -> String {
        let summary = match self.steps.as_slice() {
            [ResolvedStep::ShellRun { cmd, .. }] => cmd.clone(),
            _ => self
                .steps
                .iter()
                .map(ResolvedStep::summary)
                .collect::<Vec<_>>()
                .join(", "),
        };
        format!(
            "Run \"{}\"? {summary} · Return to run · Esc to cancel",
            self.label
        )
    }
}

/// A [`Step`] with every string filled from the match and `place` resolved to
/// a concrete [`Place`]. `workspace.open`/`close` keep their workspace name;
/// expansion into window defs happens at run time in the host's [`Actions`].
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedStep {
    /// Open the menu bar popover.
    JisunoMenu,
    /// Open the Settings window.
    JisunoSettings,
    /// Launch the app if needed, open `arg` in it.
    AppOpen { app: String, arg: Option<String> },
    /// Focus the window.
    WindowFocus { window: String },
    /// Move and resize the window.
    WindowTile { window: String, place: Place },
    /// Open each window of the workspace in turn.
    WorkspaceOpen { workspace: String },
    /// Close the workspace windows.
    WorkspaceClose { workspace: String },
    /// Direct AX lookup by app + role + name, then press.
    AxPress {
        app: String,
        role: String,
        name: String,
    },
    /// Direct AX lookup, then set the value.
    AxSetValue {
        app: String,
        role: String,
        name: String,
        value: String,
    },
    /// Send the key chord to the focused app.
    KeysSend { keys: String },
    /// Run the shell command (always behind a confirm).
    ShellRun { cmd: String, cwd: Option<String> },
    /// Switch the theme.
    ThemeSet { theme: String },
    /// Start loading the model.
    ModelWarm,
    /// Unload the model from memory.
    ModelUnload,
    /// Pause the runner: the host sleeps/polls like any other step.
    Wait {
        window: Option<String>,
        ms: Option<u64>,
    },
}

impl ResolvedStep {
    /// The `do` action name, used in failure HUD copy.
    pub fn action(&self) -> &'static str {
        match self {
            Self::JisunoMenu => "jisuno.menu",
            Self::JisunoSettings => "jisuno.settings",
            Self::AppOpen { .. } => "app.open",
            Self::WindowFocus { .. } => "window.focus",
            Self::WindowTile { .. } => "window.tile",
            Self::WorkspaceOpen { .. } => "workspace.open",
            Self::WorkspaceClose { .. } => "workspace.close",
            Self::AxPress { .. } => "ax.press",
            Self::AxSetValue { .. } => "ax.set_value",
            Self::KeysSend { .. } => "keys.send",
            Self::ShellRun { .. } => "shell.run",
            Self::ThemeSet { .. } => "theme.set",
            Self::ModelWarm => "model.warm",
            Self::ModelUnload => "model.unload",
            Self::Wait { .. } => "wait",
        }
    }

    /// Short one-line summary in the style of the `jisuno control list`
    /// "steps" column (e.g. `app.open Mail`).
    pub fn summary(&self) -> String {
        match self {
            Self::JisunoMenu | Self::JisunoSettings | Self::ModelWarm | Self::ModelUnload => {
                self.action().to_string()
            }
            Self::AppOpen { app, .. } => format!("app.open {app}"),
            Self::WindowFocus { window } => format!("window.focus {window}"),
            Self::WindowTile { window, place } => {
                format!("window.tile {window} {}", place.as_str())
            }
            Self::WorkspaceOpen { workspace } => format!("workspace.open {workspace}"),
            Self::WorkspaceClose { workspace } => format!("workspace.close {workspace}"),
            Self::AxPress { app, role, name } => format!("ax.press {app} {role} {name}"),
            Self::AxSetValue {
                app, role, name, ..
            } => format!("ax.set_value {app} {role} {name}"),
            Self::KeysSend { keys } => format!("keys.send {keys}"),
            Self::ShellRun { cmd, .. } => format!("shell.run {cmd}"),
            Self::ThemeSet { theme } => format!("theme.set {theme}"),
            Self::Wait {
                window: Some(window),
                ..
            } => format!("wait {window}"),
            Self::Wait { ms: Some(ms), .. } => format!("wait {ms}ms"),
            Self::Wait { .. } => "wait".to_string(),
        }
    }
}

/// The macOS host implements the actual side effects. `Err` is the failure
/// reason text shown in the HUD (e.g. `no window "terminal"`).
pub trait Actions {
    /// Run one resolved step. `workspaces` is the config's workspace layouts
    /// for expanding `workspace.open`/`close`.
    fn run(
        &mut self,
        step: &ResolvedStep,
        workspaces: &BTreeMap<String, Vec<WindowDef>>,
    ) -> Result<(), String>;
}

/// What [`execute`] stopped with.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Every step ran.
    Done,
    /// The step at `step_index` (1-based) failed; earlier steps stay done,
    /// later steps never run. `hud` is exactly
    /// `Step <i> of <total> failed: <action> · <reason>`.
    Failed {
        step_index: usize,
        total: usize,
        action: String,
        reason: String,
        hud: String,
    },
}

/// Run a plan's steps in order. The first failure stops the command with no
/// rollback. `wait` steps are passed to [`Actions`] like any other step; the
/// host sleeps or polls.
pub fn execute(
    plan: &Plan,
    actions: &mut impl Actions,
    workspaces: &BTreeMap<String, Vec<WindowDef>>,
) -> Outcome {
    let total = plan.steps.len();
    for (index, step) in plan.steps.iter().enumerate() {
        if let Err(reason) = actions.run(step, workspaces) {
            let action = step.action().to_string();
            return Outcome::Failed {
                hud: format!("Step {} of {total} failed: {action} · {reason}", index + 1),
                step_index: index + 1,
                total,
                action,
                reason,
            };
        }
    }
    Outcome::Done
}

/// Build a [`Plan`] by filling every step from the match's slot map.
fn fill_plan(
    command_id: &str,
    label: &str,
    steps: &[Step],
    slots: &BTreeMap<String, String>,
) -> Result<Plan, String> {
    let mut resolved = Vec::with_capacity(steps.len());
    for step in steps {
        resolved.push(fill_step(step, slots)?);
    }
    Ok(Plan {
        command_id: command_id.to_string(),
        label: label.to_string(),
        steps: resolved,
    })
}

/// Fill one step's string arguments; `place` slot refs resolve to a concrete
/// [`Place`].
fn fill_step(step: &Step, slots: &BTreeMap<String, String>) -> Result<ResolvedStep, String> {
    match step {
        Step::JisunoMenu => Ok(ResolvedStep::JisunoMenu),
        Step::JisunoSettings => Ok(ResolvedStep::JisunoSettings),
        Step::AppOpen { app, arg } => Ok(ResolvedStep::AppOpen {
            app: fill_string(app, slots)?,
            arg: arg
                .as_ref()
                .map(|value| fill_string(value, slots))
                .transpose()?,
        }),
        Step::WindowFocus { window } => Ok(ResolvedStep::WindowFocus {
            window: fill_string(window, slots)?,
        }),
        Step::WindowTile { window, place } => {
            let window = fill_string(window, slots)?;
            let place = match place {
                PlaceOrSlot::Place(place) => *place,
                PlaceOrSlot::Slot(reference) => parse_place(&fill_string(reference, slots)?)
                    .ok_or_else(|| {
                        format!(
                            "slot value is not a valid place: {}",
                            fill_string(reference, slots).unwrap_or_else(|_| reference.clone())
                        )
                    })?,
            };
            Ok(ResolvedStep::WindowTile { window, place })
        }
        Step::WorkspaceOpen { workspace } => Ok(ResolvedStep::WorkspaceOpen {
            workspace: fill_string(workspace, slots)?,
        }),
        Step::WorkspaceClose { workspace } => Ok(ResolvedStep::WorkspaceClose {
            workspace: fill_string(workspace, slots)?,
        }),
        Step::AxPress { app, role, name } => Ok(ResolvedStep::AxPress {
            app: fill_string(app, slots)?,
            role: fill_string(role, slots)?,
            name: fill_string(name, slots)?,
        }),
        Step::AxSetValue {
            app,
            role,
            name,
            value,
        } => Ok(ResolvedStep::AxSetValue {
            app: fill_string(app, slots)?,
            role: fill_string(role, slots)?,
            name: fill_string(name, slots)?,
            value: fill_string(value, slots)?,
        }),
        Step::KeysSend { keys } => Ok(ResolvedStep::KeysSend {
            keys: fill_string(keys, slots)?,
        }),
        Step::ShellRun { cmd, cwd } => Ok(ResolvedStep::ShellRun {
            cmd: fill_string(cmd, slots)?,
            cwd: cwd
                .as_ref()
                .map(|value| fill_string(value, slots))
                .transpose()?,
        }),
        Step::ThemeSet { theme } => Ok(ResolvedStep::ThemeSet {
            theme: fill_string(theme, slots)?,
        }),
        Step::ModelWarm => Ok(ResolvedStep::ModelWarm),
        Step::ModelUnload => Ok(ResolvedStep::ModelUnload),
        Step::Wait { window, ms } => Ok(ResolvedStep::Wait {
            window: window
                .as_ref()
                .map(|value| fill_string(value, slots))
                .transpose()?,
            ms: *ms,
        }),
    }
}

/// Substitute every `{slot}` ref in `template` with the matched value.
/// Text in braces that is not a slot name (e.g. shell `{a,b}`) is left alone;
/// a well-formed ref with no matched value is an error.
fn fill_string(template: &str, slots: &BTreeMap<String, String>) -> Result<String, String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let (before, from_brace) = rest.split_at(open);
        out.push_str(before);
        match from_brace.find('}') {
            Some(close) => {
                let name = &from_brace[1..close];
                if is_slot_name(name) {
                    match slots.get(name) {
                        Some(value) => out.push_str(value),
                        None => return Err(format!("no matched value for slot \"{name}\"")),
                    }
                    rest = &from_brace[close + 1..];
                } else {
                    out.push('{');
                    rest = &from_brace[1..];
                }
            }
            None => {
                out.push_str(from_brace);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    Ok(out)
}

fn is_slot_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first == '_' || first.is_ascii_lowercase() => (),
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Parse a filled `place` value into its [`Place`]. Accepts the wire spelling
/// (`"right-half"`) and the spoken form (`"right half"`).
fn parse_place(value: &str) -> Option<Place> {
    match value.trim().to_lowercase().replace(' ', "-").as_str() {
        "left-half" => Some(Place::LeftHalf),
        "right-half" => Some(Place::RightHalf),
        "top-half" => Some(Place::TopHalf),
        "bottom-half" => Some(Place::BottomHalf),
        "left-top" => Some(Place::LeftTop),
        "left-bottom" => Some(Place::LeftBottom),
        "right-top" => Some(Place::RightTop),
        "right-bottom" => Some(Place::RightBottom),
        "full" => Some(Place::Full),
        "center" => Some(Place::Center),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice_control;

    /// The `voice-control.md` §2 example, comments stripped: `//` to end of
    /// line is cut only outside string literals so URLs survive.
    const EXAMPLE_JSONC: &str = r#"{
  "$schema": "./schemas/voice-control-config.schema.json",
  "version": 1,
  "commands": [
    {"id": "menu", "say": ["open jisuno"], "steps": [{"do": "jisuno.menu"}]},
    {"id": "settings", "say": ["open settings"], "steps": [{"do": "jisuno.settings"}]},
    {"id": "work.start", "say": ["start working"], "steps": [{"do": "workspace.open", "workspace": "work"}]},
    {"id": "work.stop", "say": ["stop working"], "steps": [{"do": "workspace.close", "workspace": "work"}]},
    {"id": "focus", "say": ["focus {window}"], "steps": [{"do": "window.focus", "window": "{window}"}]},
    {"id": "move", "say": ["move {window} to {place}"], "steps": [{"do": "window.tile", "window": "{window}", "place": "{place}"}]},
    {"id": "note", "say": ["open note {name}"], "steps": [{"do": "app.open", "app": "Inkuno", "arg": "{name}.md"}]},
    // ^ {name} is the built-in free-text slot: one per phrase, last token.
    {"id": "send", "say": ["send it"], "steps": [{"do": "ax.press", "app": "focused", "role": "AXButton", "name": "Send"}]},
    {"id": "clean", "say": ["clean build"], "confirm": true, "steps": [{"do": "shell.run", "cmd": "cargo clean"}]},
    // ^ shell.run requires "confirm": true, rejected otherwise.
    {"id": "dark", "say": ["dark mode"], "steps": [{"do": "theme.set", "theme": "quiet-dark"}]},
    {"id": "light", "say": ["light mode"], "steps": [{"do": "theme.set", "theme": "quiet-light"}]},
    {"id": "reply", "say": ["reply"], "when": {"app": ["com.apple.mail"]},
     "steps": [{"do": "ax.press", "app": "focused", "role": "AXButton", "name": "Reply"}]},
    // ^ "when": only matches while Mail is frontmost. Filtering happens before matching.
    {"id": "project", "say": ["open project {project}"],
     "slots": {"project": ["jisuno", "inkuno", "rex"]},
     "steps": [{"do": "app.open", "app": "Zed", "arg": "~/github/{project}"}]}
    // ^ declared fixed-list slot: the value list lives in the command.
  ],
  "workspaces": {
    "work": [
      {"id": "terminal", "app": "Rex", "title": "hoocode — ~/github/jisuno", "run": "hoocode", "place": "left-half"},
      {"id": "editor", "app": "Zed", "title": "jisuno — Zed", "open": "~/github/jisuno", "place": "right-top"},
      {"id": "browser", "app": "Safari", "title": "localhost:4000", "open": "http://localhost:4000", "place": "right-bottom"}
    ]
  }
}"#;

    fn strip_jsonc_comments(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for line in text.lines() {
            let mut in_string = false;
            let mut escaped = false;
            let mut cut = line.len();
            let bytes = line.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                let byte = bytes[i];
                if in_string {
                    if escaped {
                        escaped = false;
                    } else if byte == b'\\' {
                        escaped = true;
                    } else if byte == b'"' {
                        in_string = false;
                    }
                } else if byte == b'"' {
                    in_string = true;
                } else if byte == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                    cut = i;
                    break;
                }
                i += 1;
            }
            out.push_str(line[..cut].trim_end());
            out.push('\n');
        }
        out
    }

    fn fixture() -> (Commands, BTreeMap<String, Vec<WindowDef>>) {
        let stripped = strip_jsonc_comments(EXAMPLE_JSONC);
        assert!(
            stripped.contains("http://localhost:4000"),
            "fixture must keep the URL intact: {stripped}"
        );
        let (config, warnings) =
            voice_control::parse(&stripped).expect("example config must validate");
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        let runtime = Runtime {
            windows: vec!["Mail".to_string(), "Finder".to_string()],
            themes: vec!["quiet-dark".to_string(), "quiet-light".to_string()],
            apps: vec![
                "Inkuno".to_string(),
                "Zed".to_string(),
                "Mail".to_string(),
                "Safari".to_string(),
                "Rex".to_string(),
            ],
        };
        let workspaces = config.workspaces.clone();
        (Commands::new(&config, &runtime), workspaces)
    }

    #[test]
    fn resolve_exact_phrase() {
        let (commands, _) = fixture();
        match commands.resolve("open settings", None) {
            Resolution::Run(plan) => {
                assert_eq!(plan.command_id, "settings");
                assert_eq!(plan.label, "open settings");
                assert_eq!(plan.steps, vec![ResolvedStep::JisunoSettings]);
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn resolve_fixed_slots_fill_window_tile() {
        let (commands, _) = fixture();
        match commands.resolve("move terminal to right half", None) {
            Resolution::Run(plan) => {
                assert_eq!(plan.command_id, "move");
                assert_eq!(plan.label, "move terminal to right half");
                assert_eq!(
                    plan.steps,
                    vec![ResolvedStep::WindowTile {
                        window: "terminal".to_string(),
                        place: Place::RightHalf,
                    }]
                );
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn resolve_free_slot_fills_embedded_arg() {
        let (commands, _) = fixture();
        match commands.resolve("open note standup", None) {
            Resolution::Run(plan) => {
                assert_eq!(plan.command_id, "note");
                assert_eq!(
                    plan.steps,
                    vec![ResolvedStep::AppOpen {
                        app: "Inkuno".to_string(),
                        arg: Some("standup.md".to_string()),
                    }]
                );
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn resolve_declared_list_slot() {
        let (commands, _) = fixture();
        match commands.resolve("open project jisuno", None) {
            Resolution::Run(plan) => {
                assert_eq!(plan.command_id, "project");
                assert_eq!(
                    plan.steps,
                    vec![ResolvedStep::AppOpen {
                        app: "Zed".to_string(),
                        arg: Some("~/github/jisuno".to_string()),
                    }]
                );
            }
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn when_filter_limits_to_frontmost_app() {
        let (commands, _) = fixture();
        match commands.resolve("reply", Some("com.apple.mail")) {
            Resolution::Run(plan) => assert_eq!(plan.command_id, "reply"),
            other => panic!("expected Run under Mail, got {other:?}"),
        }
        // Under another app the `reply` command is filtered out before matching,
        // so the matcher falls through to a suggestion instead of running it.
        assert!(matches!(
            commands.resolve("reply", Some("com.apple.safari")),
            Resolution::DidYouMean(_)
        ));
        assert!(matches!(
            commands.resolve("reply", None),
            Resolution::DidYouMean(_)
        ));
        assert_eq!(commands.resolve("", None), Resolution::NoMatch);
    }

    #[test]
    fn confirm_command_needs_prompt_with_exact_copy() {
        let (commands, _) = fixture();
        match commands.resolve("clean build", None) {
            Resolution::Confirm(plan) => {
                assert_eq!(plan.command_id, "clean");
                assert_eq!(plan.label, "clean build");
                assert_eq!(
                    plan.confirm_prompt(),
                    "Run \"clean build\"? cargo clean · Return to run · Esc to cancel"
                );
            }
            other => panic!("expected Confirm, got {other:?}"),
        }
    }

    #[test]
    fn non_shell_confirm_prompt_uses_step_summary() {
        let plan = Plan {
            command_id: "note".to_string(),
            label: "open note standup".to_string(),
            steps: vec![ResolvedStep::AppOpen {
                app: "Inkuno".to_string(),
                arg: Some("standup.md".to_string()),
            }],
        };
        assert_eq!(
            plan.confirm_prompt(),
            "Run \"open note standup\"? app.open Inkuno · Return to run · Esc to cancel"
        );
    }

    #[test]
    fn did_you_mean_copy() {
        let (commands, _) = fixture();
        match commands.resolve("cancel everything right now", None) {
            Resolution::DidYouMean(copy) => {
                assert!(
                    copy.starts_with("Did you mean \"") && copy.ends_with("\"?"),
                    "bad HUD copy: {copy}"
                );
                assert!(copy.len() > "Did you mean \"\"?".len());
            }
            other => panic!("expected DidYouMean, got {other:?}"),
        }
    }

    struct Script {
        fail_on: Option<usize>,
        reason: String,
        ran: Vec<String>,
    }

    impl Script {
        fn ok() -> Self {
            Self {
                fail_on: None,
                reason: String::new(),
                ran: Vec::new(),
            }
        }

        fn fail_on(fail_on: usize, reason: &str) -> Self {
            Self {
                fail_on: Some(fail_on),
                reason: reason.to_string(),
                ran: Vec::new(),
            }
        }
    }

    impl Actions for Script {
        fn run(
            &mut self,
            step: &ResolvedStep,
            _workspaces: &BTreeMap<String, Vec<WindowDef>>,
        ) -> Result<(), String> {
            self.ran.push(step.action().to_string());
            if self.fail_on == Some(self.ran.len()) {
                return Err(self.reason.clone());
            }
            Ok(())
        }
    }

    fn three_step_plan() -> Plan {
        Plan {
            command_id: "demo".to_string(),
            label: "demo".to_string(),
            steps: vec![
                ResolvedStep::JisunoMenu,
                ResolvedStep::WindowTile {
                    window: "terminal".to_string(),
                    place: Place::RightHalf,
                },
                ResolvedStep::AppOpen {
                    app: "Inkuno".to_string(),
                    arg: None,
                },
            ],
        }
    }

    #[test]
    fn execute_success_runs_every_step() {
        let plan = three_step_plan();
        let mut actions = Script::ok();
        let workspaces = BTreeMap::new();
        assert_eq!(execute(&plan, &mut actions, &workspaces), Outcome::Done);
        assert_eq!(actions.ran, vec!["jisuno.menu", "window.tile", "app.open"]);
    }

    #[test]
    fn execute_failure_reports_exact_hud() {
        let plan = three_step_plan();
        let mut actions = Script::fail_on(2, "no window \"terminal\"");
        let workspaces = BTreeMap::new();
        assert_eq!(
            execute(&plan, &mut actions, &workspaces),
            Outcome::Failed {
                step_index: 2,
                total: 3,
                action: "window.tile".to_string(),
                reason: "no window \"terminal\"".to_string(),
                hud: "Step 2 of 3 failed: window.tile · no window \"terminal\"".to_string(),
            }
        );
    }

    #[test]
    fn execute_stops_after_first_failure() {
        let plan = three_step_plan();
        let mut actions = Script::fail_on(2, "no window \"terminal\"");
        let workspaces = BTreeMap::new();
        let _ = execute(&plan, &mut actions, &workspaces);
        assert_eq!(actions.ran, vec!["jisuno.menu", "window.tile"]);
    }
}
