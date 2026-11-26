//! `jisuno control`: the agent CLI for `voice-control-config.json`
//! (`docs/design/voice-control.md` §8).
//!
//! Every write validates the whole resulting file with `voice_control::parse`
//! first and lands through `write_atomic`. Exit codes: 0 ok, 2 validation
//! error (nothing written), 1 other error (no file, unknown id, bad flags).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use jisuno_logic::settings::{write_atomic, ConfigDir, EmbeddedFile};
use jisuno_logic::voice_control::{self, CommandDef, Config, Diagnostic, Step};
use serde_json::{json, Value};

const FILE_NAME: &str = "voice-control-config.json";
const DEFAULTS_TEXT: &str = include_str!("../../jisuno-logic/defaults/voice-control-config.json");
const SCHEMA_PATH: &str = "schemas/voice-control-config.schema.json";
const SCHEMA_TEXT: &str =
    include_str!("../../jisuno-logic/schemas/voice-control-config.schema.json");

const EXIT_OK: i32 = 0;
const EXIT_OTHER: i32 = 1;
const EXIT_INVALID: i32 = 2;

#[derive(Subcommand)]
pub enum ControlCommand {
    /// List commands: id, phrases, steps (table, or the commands as JSON).
    List {
        /// Print the commands array as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Add a one-step command, tagged `"agent": true`.
    Add(Box<AddArgs>),
    /// Remove a command by id.
    Remove {
        /// Id of the command to remove.
        id: String,
    },
    /// Check the config file and print errors and warnings.
    Validate,
}

#[derive(Args)]
pub struct AddArgs {
    /// Command id, unique across commands.
    #[arg(long)]
    id: String,
    /// Spoken phrase. Repeatable.
    #[arg(long = "say", required = true)]
    say: Vec<String>,
    /// Action, e.g. `app.open` (§4).
    #[arg(long = "do")]
    action: String,
    /// Require confirmation before the steps run.
    #[arg(long)]
    confirm: bool,
    /// Frontmost-app bundle id the command is limited to. Repeatable.
    #[arg(long = "when-app")]
    when_app: Vec<String>,
    /// Action argument, valid only for actions that take it (§4).
    #[arg(long)]
    app: Option<String>,
    #[arg(long)]
    arg: Option<String>,
    #[arg(long)]
    window: Option<String>,
    #[arg(long)]
    place: Option<String>,
    #[arg(long)]
    workspace: Option<String>,
    #[arg(long)]
    role: Option<String>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    value: Option<String>,
    #[arg(long)]
    keys: Option<String>,
    #[arg(long)]
    cmd: Option<String>,
    #[arg(long)]
    cwd: Option<String>,
    #[arg(long)]
    theme: Option<String>,
    #[arg(long)]
    ms: Option<u64>,
}

impl AddArgs {
    /// The action arguments that were given, in §4 order.
    fn given_args(&self) -> Vec<(&'static str, Value)> {
        let text: [(&'static str, &Option<String>); 12] = [
            ("app", &self.app),
            ("arg", &self.arg),
            ("window", &self.window),
            ("place", &self.place),
            ("workspace", &self.workspace),
            ("role", &self.role),
            ("name", &self.name),
            ("value", &self.value),
            ("keys", &self.keys),
            ("cmd", &self.cmd),
            ("cwd", &self.cwd),
            ("theme", &self.theme),
        ];
        let mut given: Vec<(&'static str, Value)> = text
            .iter()
            .filter_map(|(key, value)| value.as_ref().map(|v| (*key, Value::from(v.as_str()))))
            .collect();
        if let Some(ms) = self.ms {
            given.push(("ms", Value::from(ms)));
        }
        given
    }
}

/// Run a `control` subcommand and return the process exit code.
pub fn run(command: ControlCommand) -> i32 {
    let result = match command {
        ControlCommand::List { json } => list(json),
        ControlCommand::Add(args) => add(&args),
        ControlCommand::Remove { id } => remove(&id),
        ControlCommand::Validate => validate(),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            EXIT_OTHER
        }
    }
}

fn list(json: bool) -> Result<i32> {
    let Some((_, text)) = load()? else {
        return Ok(EXIT_OTHER);
    };
    let (config, _) = match voice_control::parse(&text) {
        Ok(parsed) => parsed,
        Err(errors) => return Ok(reject_file(&errors)),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&config.commands)?);
    } else {
        print_table(&config.commands);
    }
    Ok(EXIT_OK)
}

fn add(args: &AddArgs) -> Result<i32> {
    // Usage errors: flags the action does not take. An unknown action is
    // left to validation (§7 check 9).
    if let Some(allowed) = action_args(&args.action) {
        for (flag, _) in args.given_args() {
            if !allowed.contains(&flag) {
                eprintln!(
                    "error: unexpected argument \"--{flag}\" for \"{}\"",
                    args.action
                );
                return Ok(EXIT_OTHER);
            }
        }
    }

    let dir = ConfigDir::resolve()?;
    let path = dir.voice_control_path();
    let existing = read_config(&path)?;
    // A missing file starts from the embedded defaults (§1).
    let base = existing.as_deref().unwrap_or(DEFAULTS_TEXT);
    let (config, _) = match voice_control::parse(base) {
        Ok(parsed) => parsed,
        Err(errors) => return Ok(reject_file(&errors)),
    };

    let mut root = serde_json::to_value(&config)?;
    if let Some(commands) = root.get_mut("commands").and_then(Value::as_array_mut) {
        commands.push(new_command(args));
    }
    // Errors name the new command, so they print without the file:line prefix.
    let candidate = serde_json::to_string(&root)?;
    let config = match voice_control::parse(&candidate) {
        Ok((config, _)) => config,
        Err(errors) => {
            for error in &errors {
                eprintln!("error: {}", error.message);
            }
            return Ok(EXIT_INVALID);
        }
    };

    if existing.is_none() {
        dir.ensure_first_run(&[EmbeddedFile {
            name: SCHEMA_PATH,
            contents: SCHEMA_TEXT,
        }])?;
    }
    if save(&path, &config)? != EXIT_OK {
        return Ok(EXIT_INVALID);
    }
    println!("added \"{}\"", args.id);
    Ok(EXIT_OK)
}

fn remove(id: &str) -> Result<i32> {
    let Some((path, text)) = load()? else {
        return Ok(EXIT_OTHER);
    };
    let (mut config, _) = match voice_control::parse(&text) {
        Ok(parsed) => parsed,
        Err(errors) => return Ok(reject_file(&errors)),
    };
    let Some(index) = config.commands.iter().position(|c| c.id == id) else {
        eprintln!("error: no command \"{id}\"");
        return Ok(EXIT_OTHER);
    };
    config.commands.remove(index);
    if save(&path, &config)? != EXIT_OK {
        return Ok(EXIT_INVALID);
    }
    println!("removed \"{id}\"");
    Ok(EXIT_OK)
}

fn validate() -> Result<i32> {
    let Some((_, text)) = load()? else {
        return Ok(EXIT_OTHER);
    };
    match voice_control::parse(&text) {
        Err(errors) => Ok(reject_file(&errors)),
        Ok((_, warnings)) => {
            for warning in &warnings {
                eprintln!("warning: {}", warning.render(FILE_NAME));
            }
            println!("ok");
            Ok(EXIT_OK)
        }
    }
}

/// The config path and text. A missing file is reported here (exit 1).
fn load() -> Result<Option<(PathBuf, String)>> {
    let path = ConfigDir::resolve()?.voice_control_path();
    match read_config(&path)? {
        Some(text) => Ok(Some((path, text))),
        None => {
            eprintln!(
                "error: no {FILE_NAME} at {} (run the app once or create it)",
                path.display()
            );
            Ok(None)
        }
    }
}

/// The file text, or `None` when the file does not exist.
fn read_config(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Print validation errors for the file on stderr (§7 format), exit 2.
fn reject_file(errors: &[Diagnostic]) -> i32 {
    for error in errors {
        eprintln!("{}", error.render(FILE_NAME));
    }
    EXIT_INVALID
}

/// Validates the serialized file, then writes it. Nothing is written on error.
fn save(path: &Path, config: &Config) -> Result<i32> {
    let text = config.to_pretty_json();
    if let Err(errors) = voice_control::parse(&text) {
        return Ok(reject_file(&errors));
    }
    write_atomic(path, &text)?;
    Ok(EXIT_OK)
}

/// Argument flags an action takes (§4). `None` for an unknown action.
fn action_args(action: &str) -> Option<&'static [&'static str]> {
    Some(match action {
        "jisuno.menu" | "jisuno.settings" | "model.warm" | "model.unload" => &[],
        "app.open" => &["app", "arg"],
        "window.focus" => &["window"],
        "window.tile" => &["window", "place"],
        "workspace.open" | "workspace.close" => &["workspace"],
        "ax.press" => &["app", "role", "name"],
        "ax.set_value" => &["app", "role", "name", "value"],
        "keys.send" => &["keys"],
        "shell.run" => &["cmd", "cwd"],
        "theme.set" => &["theme"],
        "wait" => &["window", "ms"],
        _ => return None,
    })
}

/// The JSON object for the new command. Validation, not this function, decides
/// whether its arguments are complete.
fn new_command(args: &AddArgs) -> Value {
    let mut step = serde_json::Map::new();
    step.insert("do".to_string(), Value::from(args.action.as_str()));
    for (key, value) in args.given_args() {
        step.insert(key.to_string(), value);
    }
    let mut command = json!({
        "id": args.id,
        "say": args.say,
        "steps": [Value::Object(step)],
        "agent": true,
    });
    if args.confirm {
        command["confirm"] = Value::Bool(true);
    }
    if !args.when_app.is_empty() {
        command["when"] = json!({ "app": args.when_app });
    }
    command
}

/// `list` output: id, phrases, steps. Columns are padded to a multiple of 4
/// with at least two spaces after the longest entry (matches §8).
fn print_table(commands: &[CommandDef]) {
    let rows: Vec<[String; 3]> = commands
        .iter()
        .map(|c| {
            [
                c.id.clone(),
                c.say.join(", "),
                c.steps
                    .iter()
                    .map(step_summary)
                    .collect::<Vec<_>>()
                    .join("; "),
            ]
        })
        .collect();
    let width = |column: usize| {
        let longest = rows
            .iter()
            .map(|row| row[column].chars().count())
            .chain([["id", "phrases", "steps"][column].len()])
            .max()
            .unwrap_or(0);
        (longest + 2).div_ceil(4) * 4
    };
    let (id_w, phrase_w) = (width(0), width(1));
    println!("{:<id_w$}{:<phrase_w$}steps", "id", "phrases");
    for [id, phrases, steps] in &rows {
        println!("{id:<id_w$}{phrases:<phrase_w$}{steps}");
    }
}

/// `<do> <first argument value>`, e.g. `app.open Mail`.
fn step_summary(step: &Step) -> String {
    let (action, arg): (&str, Option<String>) = match step {
        Step::JisunoMenu => ("jisuno.menu", None),
        Step::JisunoSettings => ("jisuno.settings", None),
        Step::AppOpen { app, .. } => ("app.open", Some(app.clone())),
        Step::WindowFocus { window } => ("window.focus", Some(window.clone())),
        Step::WindowTile { window, .. } => ("window.tile", Some(window.clone())),
        Step::WorkspaceOpen { workspace } => ("workspace.open", Some(workspace.clone())),
        Step::WorkspaceClose { workspace } => ("workspace.close", Some(workspace.clone())),
        Step::AxPress { app, .. } => ("ax.press", Some(app.clone())),
        Step::AxSetValue { app, .. } => ("ax.set_value", Some(app.clone())),
        Step::KeysSend { keys } => ("keys.send", Some(keys.clone())),
        Step::ShellRun { cmd, .. } => ("shell.run", Some(cmd.clone())),
        Step::ThemeSet { theme } => ("theme.set", Some(theme.clone())),
        Step::ModelWarm => ("model.warm", None),
        Step::ModelUnload => ("model.unload", None),
        Step::Wait { window, ms } => ("wait", window.clone().or_else(|| ms.map(|m| m.to_string()))),
    };
    match arg {
        Some(arg) => format!("{action} {arg}"),
        None => action.to_string(),
    }
}
