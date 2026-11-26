//! `voice-control-config.json`: types, loader, and validation entry point.
//!
//! File form (see `docs/design/voice-control.md` §2–§3):
//! `{"version": 1, "commands": [{id, say, steps, ...}], "workspaces": {...}}`.
//!
//! [`parse`] validates the raw text and returns every error found, plus
//! warnings that do not fail the parse. [`defaults`] parses the embedded
//! default file and is the source of truth.

mod scan;
mod validate;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

const DEFAULTS_TEXT: &str = include_str!("../../defaults/voice-control-config.json");

/// Severity of a [`Diagnostic`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// One validation problem. `line` is the 1-based line of the offending
/// command, step, or workspace window in the source text (1 for whole-file
/// structural errors).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: usize,
    pub severity: Severity,
    pub message: String,
}

impl Diagnostic {
    fn error(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            severity: Severity::Error,
            message: message.into(),
        }
    }

    fn warning(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            severity: Severity::Warning,
            message: message.into(),
        }
    }

    /// True when this diagnostic fails the parse.
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }

    /// `<file>:<line>: <message>`, the §7 message format.
    pub fn render(&self, file_name: &str) -> String {
        format!("{file_name}:{}: {}", self.line, self.message)
    }
}

/// The validated `voice-control-config.json` document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    pub commands: Vec<CommandDef>,
    #[serde(default)]
    pub workspaces: BTreeMap<String, Vec<WindowDef>>,
}

impl Config {
    /// Serialize with 2-space indent, omitting defaults (`confirm: false`,
    /// `agent: false`, empty `slots`, absent `when`). Stable: parsing the
    /// output and serializing again yields the same string.
    pub fn to_pretty_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// One entry of `commands[]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandDef {
    pub id: String,
    pub say: Vec<String>,
    pub steps: Vec<Step>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub confirm: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<WhenApp>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub slots: BTreeMap<String, SlotDecl>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub agent: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

/// A declared slot: a fixed value list, or `"free"` for free text.
#[derive(Debug, Clone, PartialEq)]
pub enum SlotDecl {
    List(Vec<String>),
    Free,
}

impl Serialize for SlotDecl {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::List(values) => values.serialize(serializer),
            Self::Free => "free".serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for SlotDecl {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        if let Some(text) = value.as_str() {
            if text == "free" {
                return Ok(Self::Free);
            }
        }
        if let Some(items) = value.as_array() {
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                match item.as_str() {
                    Some(text) => values.push(text.to_string()),
                    None => {
                        return Err(serde::de::Error::custom(
                            "slot lists hold strings, or the slot is \"free\"",
                        ));
                    }
                }
            }
            return Ok(Self::List(values));
        }
        Err(serde::de::Error::custom(
            "slot must be a value list or \"free\"",
        ))
    }
}

/// One step in a command. One variant per §4 action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do")]
pub enum Step {
    #[serde(rename = "jisuno.menu")]
    JisunoMenu,
    #[serde(rename = "jisuno.settings")]
    JisunoSettings,
    #[serde(rename = "app.open")]
    AppOpen {
        app: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        arg: Option<String>,
    },
    #[serde(rename = "window.focus")]
    WindowFocus { window: String },
    #[serde(rename = "window.tile")]
    WindowTile { window: String, place: PlaceOrSlot },
    #[serde(rename = "workspace.open")]
    WorkspaceOpen { workspace: String },
    #[serde(rename = "workspace.close")]
    WorkspaceClose { workspace: String },
    #[serde(rename = "ax.press")]
    AxPress {
        app: String,
        role: String,
        name: String,
    },
    #[serde(rename = "ax.set_value")]
    AxSetValue {
        app: String,
        role: String,
        name: String,
        value: String,
    },
    #[serde(rename = "keys.send")]
    KeysSend { keys: String },
    #[serde(rename = "shell.run")]
    ShellRun {
        cmd: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
    #[serde(rename = "theme.set")]
    ThemeSet { theme: String },
    #[serde(rename = "model.warm")]
    ModelWarm,
    #[serde(rename = "model.unload")]
    ModelUnload,
    #[serde(rename = "wait")]
    Wait {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ms: Option<u64>,
    },
}

/// The 10 `place` values (§3), kebab-case on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Place {
    LeftHalf,
    RightHalf,
    TopHalf,
    BottomHalf,
    LeftTop,
    LeftBottom,
    RightTop,
    RightBottom,
    Full,
    Center,
}

impl Place {
    /// Wire spelling (`"left-half"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LeftHalf => "left-half",
            Self::RightHalf => "right-half",
            Self::TopHalf => "top-half",
            Self::BottomHalf => "bottom-half",
            Self::LeftTop => "left-top",
            Self::LeftBottom => "left-bottom",
            Self::RightTop => "right-top",
            Self::RightBottom => "right-bottom",
            Self::Full => "full",
            Self::Center => "center",
        }
    }

    fn from_str(text: &str) -> Option<Self> {
        match text {
            "left-half" => Some(Self::LeftHalf),
            "right-half" => Some(Self::RightHalf),
            "top-half" => Some(Self::TopHalf),
            "bottom-half" => Some(Self::BottomHalf),
            "left-top" => Some(Self::LeftTop),
            "left-bottom" => Some(Self::LeftBottom),
            "right-top" => Some(Self::RightTop),
            "right-bottom" => Some(Self::RightBottom),
            "full" => Some(Self::Full),
            "center" => Some(Self::Center),
            _ => None,
        }
    }
}

/// A `window.tile` target: a literal place, or a `{slot}` filled at match time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PlaceOrSlot {
    Place(Place),
    Slot(String),
}

/// The `when` gate: frontmost-app bundle ids this command is limited to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WhenApp {
    pub app: Vec<String>,
}

/// One window in a `workspaces` layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowDef {
    pub id: String,
    pub app: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    pub place: Place,
}

/// Normalize a phrase the way the matcher does (§5): lowercase, strip
/// punctuation, split into words. `{slot}` tokens keep their spelling
/// (lowercased) so slots still resolve after normalization.
pub fn normalize(phrase: &str) -> Vec<String> {
    let mut words = Vec::new();
    for token in phrase.split_whitespace() {
        let lower = token.to_lowercase();
        if is_slot_token(&lower) {
            words.push(lower);
            continue;
        }
        let stripped: String = lower.chars().filter(|c| c.is_alphanumeric()).collect();
        if !stripped.is_empty() {
            words.push(stripped);
        }
    }
    words
}

/// A full `{name}`-style token (no surrounding text).
pub(crate) fn is_slot_token(token: &str) -> bool {
    token.len() > 2 && token.starts_with('{') && token.ends_with('}')
}

/// The default config: the source of truth (§1).
/// Panics only if the embedded default file itself fails validation.
pub fn defaults() -> anyhow::Result<Config> {
    parse(DEFAULTS_TEXT)
        .map(|(config, _)| config)
        .map_err(|errors| {
            let rendered: Vec<String> = errors
                .iter()
                .map(|e| e.render("voice-control-config.json"))
                .collect();
            anyhow::anyhow!("embedded default voice-control config is invalid: {rendered:?}")
        })
}

/// Validate `text`, returning the config plus warnings. Warnings (only check
/// 21 near-duplicates) do not fail the parse. Any error rejects the whole
/// file; on error only the errors are returned.
pub fn parse(text: &str) -> Result<(Config, Vec<Diagnostic>), Vec<Diagnostic>> {
    let value: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => {
            return Err(vec![Diagnostic::error(
                error.line(),
                format!("invalid JSON: {error}"),
            )]);
        }
    };
    let map = scan::scan(text);
    let (mut errors, warnings) = validate::validate(&value, &map);
    if errors.is_empty() {
        Ok((validate::convert(&value), warnings))
    } else {
        errors.sort_by(|a, b| (a.line, &a.message).cmp(&(b.line, &b.message)));
        Err(errors)
    }
}
