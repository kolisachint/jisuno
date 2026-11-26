//! Keybindings source, validation, and display.
//!
//! File form (see `docs/design/keybindings.md` and `schemas/keybindings.schema.json`):
//! `{"$schema": ..., "bindings": [{key, hold?, when?, command}]}`.
//!
//! [`defaults`] parses the embedded default file and is the source of truth.
//! [`parse`] validates a file, returning every error found; common app-shortcut
//! collisions are warnings (via [`parse_with_warnings`]) and do not fail the parse.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

const DEFAULTS_TEXT: &str = include_str!("../defaults/keybindings.json");

/// Severity of a [`Diagnostic`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// One validation problem. `line` is the 1-based line of the offending
/// binding object in the source text (1 for whole-file structural errors).
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
}

/// A plain modifier key, in canonical `ctrl+option+shift+cmd` order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Modifier {
    Ctrl,
    Opt,
    Shift,
    Cmd,
}

impl Modifier {
    fn rank(self) -> u8 {
        match self {
            Self::Ctrl => 0,
            Self::Opt => 1,
            Self::Shift => 2,
            Self::Cmd => 3,
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Ctrl => "⌃",
            Self::Opt => "⌥",
            Self::Shift => "⇧",
            Self::Cmd => "⌘",
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            Self::Ctrl => "ctrl",
            Self::Opt => "option",
            Self::Shift => "shift",
            Self::Cmd => "cmd",
        }
    }

    fn from_token(token: &str) -> Option<Self> {
        match token {
            "ctrl" => Some(Self::Ctrl),
            "option" => Some(Self::Opt),
            "shift" => Some(Self::Shift),
            "cmd" => Some(Self::Cmd),
            _ => None,
        }
    }
}

/// A side-specific hold modifier: the only legal `hold: true` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HoldSide {
    RightCtrl,
    RightOption,
    RightShift,
    RightCmd,
}

impl HoldSide {
    fn from_token(token: &str) -> Option<Self> {
        match token {
            "right_ctrl" => Some(Self::RightCtrl),
            "right_option" => Some(Self::RightOption),
            "right_shift" => Some(Self::RightShift),
            "right_cmd" => Some(Self::RightCmd),
            _ => None,
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            Self::RightCtrl => "right_ctrl",
            Self::RightOption => "right_option",
            Self::RightShift => "right_shift",
            Self::RightCmd => "right_cmd",
        }
    }

    fn display(self) -> &'static str {
        match self {
            Self::RightCtrl => "Right ⌃",
            Self::RightOption => "Right ⌥",
            Self::RightShift => "Right ⇧",
            Self::RightCmd => "Right ⌘",
        }
    }
}

/// A parsed key: either a hold modifier or a modifier chord plus one key.
///
/// Chord equality is order-independent: modifiers compare as a set, so a key
/// built from a recording compares equal no matter the press order.
/// [`Key::parse`] only accepts canonical `ctrl+option+shift+cmd` order, so
/// parsed keys already store modifiers sorted.
#[derive(Debug, Clone)]
pub enum Key {
    Hold(HoldSide),
    Chord {
        modifiers: Vec<Modifier>,
        key: String,
    },
}

impl Key {
    /// Parse the canonical file spelling (`ctrl+option+w`, `right_option`).
    /// This is the constructor for the Settings recorder: it accepts only
    /// canonical `ctrl+option+shift+cmd` order and lowercase (§6, §8).
    pub fn parse(raw: &str) -> Result<Key, String> {
        parse_key(raw)
    }

    /// Canonical file spelling (`ctrl+option+w`, `right_option`).
    pub fn to_file_string(&self) -> String {
        self.file()
    }

    /// Display form per the §6 table (`⌃⌥W`, `Right ⌥`, `Space`, `Esc`, `⌃⌥,`).
    pub fn display(&self) -> String {
        match self {
            Self::Hold(side) => side.display().to_string(),
            Self::Chord { modifiers, key } => {
                let mut out = String::new();
                for m in modifiers {
                    out.push_str(m.glyph());
                }
                out.push_str(&key_token_display(key));
                out
            }
        }
    }

    /// Canonical file spelling (`ctrl+option+w`, `right_option`).
    pub fn file(&self) -> String {
        match self {
            Self::Hold(side) => side.file_name().to_string(),
            Self::Chord { modifiers, key } => {
                let mut parts: Vec<&str> = modifiers.iter().map(|m| m.file_name()).collect();
                parts.push(key.as_str());
                parts.join("+")
            }
        }
    }
}

impl Serialize for Key {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.file())
    }
}

impl<'de> Deserialize<'de> for Key {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        parse_key(&raw).map_err(de::Error::custom)
    }
}

/// Modifiers sorted into canonical `ctrl+option+shift+cmd` order.
fn sorted_modifiers(modifiers: &[Modifier]) -> Vec<Modifier> {
    let mut out = modifiers.to_vec();
    out.sort_by_key(|m| m.rank());
    out.dedup();
    out
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Key::Hold(a), Key::Hold(b)) => a == b,
            (
                Key::Chord {
                    modifiers: a,
                    key: ka,
                },
                Key::Chord {
                    modifiers: b,
                    key: kb,
                },
            ) => ka == kb && sorted_modifiers(a) == sorted_modifiers(b),
            _ => false,
        }
    }
}

impl Eq for Key {}

impl Hash for Key {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Key::Hold(side) => {
                0u8.hash(state);
                side.hash(state);
            }
            Key::Chord { modifiers, key } => {
                1u8.hash(state);
                sorted_modifiers(modifiers).hash(state);
                key.hash(state);
            }
        }
    }
}

fn key_token_display(token: &str) -> String {
    match token {
        "space" => "Space".to_string(),
        "escape" => "Esc".to_string(),
        "return" => "Return".to_string(),
        "tab" => "Tab".to_string(),
        "delete" => "Delete".to_string(),
        _ => token.to_uppercase(),
    }
}

/// Context a binding lives in; absent means global.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum When {
    Listening,
    Confirming,
}

impl When {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Listening => "listening",
            Self::Confirming => "confirming",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "listening" => Some(Self::Listening),
            "confirming" => Some(Self::Confirming),
            _ => None,
        }
    }
}

/// Every bindable command id (§10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Command {
    #[serde(rename = "voice.dictate")]
    Dictate,
    #[serde(rename = "voice.command")]
    VoiceCommand,
    #[serde(rename = "voice.cancel")]
    Cancel,
    #[serde(rename = "voice.confirm")]
    Confirm,
    #[serde(rename = "voice.hands_free_toggle")]
    HandsFreeToggle,
    #[serde(rename = "model.warm")]
    Warm,
    #[serde(rename = "model.unload")]
    Unload,
    #[serde(rename = "jisuno.settings")]
    Settings,
    #[serde(rename = "jisuno.menu")]
    Menu,
}

impl Command {
    /// Dotted command id used in the file.
    /// Machine id from the §10 command list (`voice.dictate`).
    pub fn as_str(self) -> &'static str {
        self.id()
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Dictate => "voice.dictate",
            Self::VoiceCommand => "voice.command",
            Self::Cancel => "voice.cancel",
            Self::Confirm => "voice.confirm",
            Self::HandsFreeToggle => "voice.hands_free_toggle",
            Self::Warm => "model.warm",
            Self::Unload => "model.unload",
            Self::Settings => "jisuno.settings",
            Self::Menu => "jisuno.menu",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        match id {
            "voice.dictate" => Some(Self::Dictate),
            "voice.command" => Some(Self::VoiceCommand),
            "voice.cancel" => Some(Self::Cancel),
            "voice.confirm" => Some(Self::Confirm),
            "voice.hands_free_toggle" => Some(Self::HandsFreeToggle),
            "model.warm" => Some(Self::Warm),
            "model.unload" => Some(Self::Unload),
            "jisuno.settings" => Some(Self::Settings),
            "jisuno.menu" => Some(Self::Menu),
            _ => None,
        }
    }
}

/// One validated binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub key: Key,
    pub hold: bool,
    #[serde(default)]
    pub when: Option<When>,
    pub command: Command,
}

/// The validated binding list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keybindings {
    pub bindings: Vec<Binding>,
}

impl Keybindings {
    /// Command for `key` in context `when`. A global binding also applies when
    /// a context is active, so an exact match wins and a global binding is the
    /// fallback. `None` as `when` only matches global bindings.
    pub fn lookup(&self, key: &Key, when: Option<When>) -> Option<Command> {
        self.conflict(key, when).map(|b| b.command).or_else(|| {
            if when.is_some() {
                self.conflict(key, None).map(|b| b.command)
            } else {
                None
            }
        })
    }

    /// Exact key+when match, for the Settings recorder conflict UI.
    /// Returns the clashing binding so the row can show
    /// `Taken by <command> (<when>)`.
    pub fn conflict(&self, key: &Key, when: Option<When>) -> Option<&Binding> {
        self.bindings
            .iter()
            .find(|b| b.key == *key && b.when == when)
    }
}

/// The default keybindings: the source of truth (§1).
/// Panics only if the embedded default file itself fails validation.
pub fn defaults() -> Keybindings {
    match parse_with_warnings(DEFAULTS_TEXT) {
        Ok((bindings, _)) => bindings,
        Err(errors) => panic!("embedded default keybindings are invalid: {errors:?}"),
    }
}

/// Validate `text`, returning every error found. Warnings are dropped; use
/// [`parse_with_warnings`] to keep them.
pub fn parse(text: &str) -> Result<Keybindings, Vec<Diagnostic>> {
    parse_with_warnings(text).map(|(bindings, _)| bindings)
}

/// Validate a single already-parsed [`Binding`] (for the Settings recorder).
/// All diagnostics use line 0; duplicates cannot be checked here.
pub fn validate_binding(binding: &Binding) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let raw = binding.key.to_file_string();
    if binding.hold {
        match &binding.key {
            Key::Hold(_) => {}
            Key::Chord { .. } if raw.contains('+') => diagnostics.push(Diagnostic::error(
                0,
                "A hold binding is one key alone · remove the chord or drop \"hold\"",
            )),
            Key::Chord { .. } => diagnostics.push(Diagnostic::error(
                0,
                "Only a single right-side modifier can be a hold key",
            )),
        }
    } else if matches!(binding.key, Key::Hold(_)) {
        diagnostics.push(Diagnostic::error(
            0,
            format!("\"{raw}\" is a hold key · set \"hold\": true"),
        ));
    }
    if diagnostics.is_empty() {
        if let Some(message) = reserved_check(&binding.key, &raw, binding.hold, binding.when) {
            diagnostics.push(Diagnostic::error(0, message));
        }
    }
    diagnostics
}

/// Validate `text`. Warnings (common app-shortcut collisions, §8) do not fail
/// the parse: they are returned alongside the bindings. Any error rejects the
/// whole file.
///
/// Each binding is validated from its own JSON value, so a type error in one
/// binding (say `"hold": "true"`) is reported at that binding's line and
/// the other bindings are still validated.
pub fn parse_with_warnings(text: &str) -> Result<(Keybindings, Vec<Diagnostic>), Vec<Diagnostic>> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| {
        vec![Diagnostic::error(
            1,
            format!("Invalid keybindings file: {e:#}"),
        )]
    })?;
    let Some(root) = value.as_object() else {
        return Err(vec![Diagnostic::error(
            1,
            "Invalid keybindings file: expected an object with a bindings array",
        )]);
    };

    let mut errors: Vec<Diagnostic> = Vec::new();
    for name in root.keys() {
        if name != "$schema" && name != "bindings" {
            errors.push(Diagnostic::error(1, format!("Unknown field \"{name}\"")));
        }
    }
    let Some(bindings_value) = root.get("bindings") else {
        errors.push(Diagnostic::error(1, "Missing \"bindings\""));
        return Err(errors);
    };
    let Some(items) = bindings_value.as_array() else {
        errors.push(Diagnostic::error(1, "\"bindings\" must be an array"));
        return Err(errors);
    };

    let lines = binding_lines(text);

    let mut warnings: Vec<Diagnostic> = Vec::new();
    let mut bindings: Vec<Binding> = Vec::new();
    let mut seen: HashMap<(Key, Option<When>), Command> = HashMap::new();

    for (index, item) in items.iter().enumerate() {
        let line = lines.get(index).copied().unwrap_or(1);
        let before = errors.len();

        let Some(object) = item.as_object() else {
            errors.push(Diagnostic::error(line, "Binding must be an object"));
            continue;
        };
        for name in object.keys() {
            if !matches!(name.as_str(), "key" | "hold" | "when" | "command") {
                errors.push(Diagnostic::error(
                    line,
                    format!("Unknown field \"{name}\" in binding"),
                ));
            }
        }

        let key_raw: Option<&str> = match object.get("key") {
            None => {
                errors.push(Diagnostic::error(line, "Binding is missing \"key\""));
                None
            }
            Some(value) => match value.as_str() {
                Some(key) => Some(key),
                None => {
                    errors.push(Diagnostic::error(line, "\"key\" must be a string"));
                    None
                }
            },
        };
        let command_raw: Option<&str> = match object.get("command") {
            None => {
                errors.push(Diagnostic::error(line, "Binding is missing \"command\""));
                None
            }
            Some(value) => match value.as_str() {
                Some(command) => Some(command),
                None => {
                    errors.push(Diagnostic::error(line, "\"command\" must be a string"));
                    None
                }
            },
        };
        let (Some(key_raw), Some(command_raw)) = (key_raw, command_raw) else {
            continue;
        };

        let hold: bool = match object.get("hold") {
            None => false,
            Some(value) => match value.as_bool() {
                Some(hold) => hold,
                None => {
                    errors.push(Diagnostic::error(line, "\"hold\" must be a boolean"));
                    continue;
                }
            },
        };

        let mut when_invalid = false;
        let when: Option<When> = match object.get("when") {
            None => None,
            Some(value) => {
                let Some(w) = value.as_str() else {
                    errors.push(Diagnostic::error(line, "\"when\" must be a string"));
                    continue;
                };
                match When::from_str(w) {
                    Some(when) => Some(when),
                    None => {
                        errors.push(Diagnostic::error(
                            line,
                            format!("Unknown when \"{w}\" · use listening or confirming"),
                        ));
                        when_invalid = true;
                        None
                    }
                }
            }
        };

        let command = match Command::from_id(command_raw) {
            Some(command) => Some(command),
            None => {
                errors.push(Diagnostic::error(
                    line,
                    format!("Unknown command \"{command_raw}\" · see the bindable commands list"),
                ));
                None
            }
        };

        let mut key: Option<Key> = None;
        if !hold && HoldSide::from_token(key_raw).is_some() {
            errors.push(Diagnostic::error(
                line,
                format!("\"{key_raw}\" is a hold key · set \"hold\": true"),
            ));
        } else if hold && key_raw.contains('+') {
            errors.push(Diagnostic::error(
                line,
                "A hold binding is one key alone · remove the chord or drop \"hold\"",
            ));
        } else {
            match parse_key(key_raw) {
                Ok(parsed) => key = Some(parsed),
                Err(message) => {
                    // A lone left modifier never parses (a modifier is not a key),
                    // but with hold it gets the specific §7 message.
                    if hold && Modifier::from_token(key_raw).is_some() {
                        errors.push(Diagnostic::error(
                        line,
                        "Hold keys must be a single right-side modifier (right_option, right_cmd, right_shift, right_ctrl)",
                    ));
                    } else {
                        errors.push(Diagnostic::error(line, message));
                    }
                }
            }
        }

        if !when_invalid {
            if let Some(parsed) = &key {
                if let Some(message) = reserved_check(parsed, key_raw, hold, when) {
                    errors.push(Diagnostic::error(line, message));
                }
            }
        }

        if errors.len() != before {
            continue;
        }

        let (Some(key), Some(command)) = (key, command) else {
            continue;
        };

        if let Some(&first) = seen.get(&(key.clone(), when)) {
            errors.push(Diagnostic::error(
                line,
                format!(
                    "Duplicate binding: \"{key_raw}\" (when: {}) is already bound to \"{}\"",
                    when.map(When::as_str).unwrap_or("global"),
                    first.id(),
                ),
            ));
            continue;
        }
        seen.insert((key.clone(), when), command);

        if when.is_none() {
            if let Key::Chord {
                modifiers,
                key: name,
            } = &key
            {
                if let Some(app_name) = common_shortcut_name(modifiers, name) {
                    warnings.push(Diagnostic::warning(
                        line,
                        format!(
                            "Warning: \"{key_raw}\" is a common app shortcut ({app_name}) · apps will stop getting it"
                        ),
                    ));
                }
            }
        }

        bindings.push(Binding {
            key,
            hold,
            when,
            command,
        });
    }

    if errors.is_empty() {
        Ok((Keybindings { bindings }, warnings))
    } else {
        Err(errors)
    }
}

/// 1-based line of each top-level object in the `bindings` array, in order.
/// serde_json has no spans, so this scans brackets while skipping strings.
fn binding_lines(text: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let bindings_at = match text.find("\"bindings\"") {
        Some(at) => at,
        None => return Vec::new(),
    };
    let array_at = match bytes[bindings_at..].iter().position(|&b| b == b'[') {
        Some(offset) => bindings_at + offset,
        None => return Vec::new(),
    };

    let mut lines = Vec::new();
    let mut bracket_depth = 0usize;
    let mut object_depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut i = array_at;
    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
        } else if b == b'"' {
            in_string = true;
        } else if b == b'[' {
            bracket_depth += 1;
        } else if b == b']' {
            if bracket_depth == 0 {
                break;
            }
            bracket_depth -= 1;
            if bracket_depth == 0 {
                break;
            }
        } else if b == b'{' {
            if bracket_depth == 1 && object_depth == 0 {
                lines.push(text[..i].matches('\n').count() + 1);
            }
            object_depth += 1;
        } else if b == b'}' {
            object_depth = object_depth.saturating_sub(1);
        }
        i += 1;
    }
    lines
}

/// Parse a key's file spelling into a [`Key`]. Error strings are the exact
/// §7/§8 messages.
fn parse_key(raw: &str) -> Result<Key, String> {
    if raw.chars().any(|c| c.is_ascii_uppercase()) {
        return Err(format!("Keys are lowercase · got \"{raw}\""));
    }
    let tokens: Vec<&str> = raw.split('+').collect();

    if tokens.iter().any(|t| *t == "fn" || *t == "globe") {
        return Err("Fn/Globe is reserved for Apple dictation · pick another key".to_string());
    }

    if tokens.len() == 1 {
        if let Some(side) = HoldSide::from_token(tokens[0]) {
            return Ok(Key::Hold(side));
        }
    }

    let mut modifiers = Vec::new();
    let mut names = Vec::new();
    for token in &tokens {
        if let Some(modifier) = Modifier::from_token(token) {
            modifiers.push(modifier);
        } else if is_key_name(token) {
            names.push(*token);
        } else {
            return Err(format!("Unknown key \"{token}\" in \"{raw}\""));
        }
    }

    // Canonical form is modifiers first (in ctrl+option+shift+cmd order) and
    // exactly one key name last: `w+ctrl` and `ctrl+w+option` fail here.
    let mut canonical = modifiers.clone();
    canonical.sort_by_key(|m| m.rank());
    canonical.dedup();
    let ordered = modifiers.len() == canonical.len() && modifiers == canonical;
    let name_last = names.len() == 1 && tokens.last() == Some(&names[0]);
    if names.len() == 1 && (!ordered || !name_last) {
        return Err(format!(
            "Modifiers must be in ctrl+option+shift+cmd order · got \"{raw}\""
        ));
    }

    match names.as_slice() {
        [name] => Ok(Key::Chord {
            modifiers: canonical,
            key: (*name).to_string(),
        }),
        [] => {
            let last = tokens.last().unwrap_or(&raw);
            Err(format!("Unknown key \"{last}\" in \"{raw}\""))
        }
        _ => Err(format!("Unknown key \"{}\" in \"{raw}\"", names[1])),
    }
}

fn is_key_name(token: &str) -> bool {
    const PUNCT: &[&str] = &[",", ".", "/", ";", "'", "[", "]", "-", "="];
    if PUNCT.contains(&token) {
        return true;
    }
    if token.len() == 1 {
        return token
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    }
    match token {
        "space" | "escape" | "return" | "tab" | "delete" => true,
        _ => is_fkey(token),
    }
}

fn is_fkey(token: &str) -> bool {
    let Some(n) = token.strip_prefix('f') else {
        return false;
    };
    let Ok(v) = n.parse::<u8>() else {
        return false;
    };
    // No leading zeros: `f05` is not `f5`.
    (1..=20).contains(&v) && format!("{v}") == n
}

/// Reserved-key and hold/context rules (§7). Only called with a parsed key.
fn reserved_check(key: &Key, raw: &str, hold: bool, when: Option<When>) -> Option<String> {
    // Hold chords never reach here: `parse` rejects `hold` + `+` up front.
    match raw {
        "cmd+space" => {
            return Some(
                "cmd+space is a macOS system shortcut (Spotlight) · use another key".to_string(),
            );
        }
        "ctrl+space" => {
            return Some(
                "ctrl+space is a macOS system shortcut (input source) · use another key"
                    .to_string(),
            );
        }
        "cmd+tab" => {
            return Some(
                "cmd+tab is a macOS system shortcut (app switcher) · use another key".to_string(),
            );
        }
        "cmd+q" => {
            return Some("cmd+q quits the frontmost app · use another key".to_string());
        }
        "cmd+h" => {
            return Some("cmd+h hides the frontmost app · use another key".to_string());
        }
        _ => {}
    }
    if hold {
        // Lone left modifiers (`ctrl` + hold) never parse, so they are handled
        // at the call site; every other parsed non-hold key lands here.
        return match key {
            Key::Hold(_) => None,
            Key::Chord { .. } => {
                Some("Only a single right-side modifier can be a hold key".to_string())
            }
        };
    }
    if when.is_none()
        && matches!(
            key,
            Key::Chord { modifiers, key }
                if modifiers.is_empty() && (key == "escape" || key == "return")
        )
    {
        return Some(
            "escape/return need a \"when\" (listening or confirming) · unscoped escape/return is rejected"
                .to_string(),
        );
    }
    if when.is_none() {
        if let Key::Chord {
            modifiers,
            key: name,
        } = key
        {
            if modifiers.is_empty() && is_typing_key(name) {
                return Some(format!(
                    "\"{raw}\" types text · add a modifier or a \"when\""
                ));
            }
        }
    }
    None
}

/// Bare keys that would steal typing when bound globally without modifiers.
fn is_typing_key(name: &str) -> bool {
    const PUNCT: &[&str] = &[",", ".", "/", ";", "'", "[", "]", "-", "="];
    if PUNCT.contains(&name) || name == "space" {
        return true;
    }
    name.len() == 1
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Common app-shortcut name for a global single-⌘ chord (§8 warning), if any.
fn common_shortcut_name(modifiers: &[Modifier], name: &str) -> Option<&'static str> {
    if modifiers != [Modifier::Cmd] {
        return None;
    }
    match name {
        "w" => Some("Close"),
        "s" => Some("Save"),
        "z" => Some("Undo"),
        "c" => Some("Copy"),
        "v" => Some("Paste"),
        "x" => Some("Cut"),
        "a" => Some("Select All"),
        "m" => Some("Minimize"),
        "," => Some("Preferences"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn single_binding(key: &str, hold: bool, when: Option<&str>, command: &str) -> String {
        let mut binding = format!("{{\"key\": \"{key}\"");
        if hold {
            binding.push_str(", \"hold\": true");
        }
        if let Some(w) = when {
            binding.push_str(&format!(", \"when\": \"{w}\""));
        }
        binding.push_str(&format!(", \"command\": \"{command}\"}}"));
        format!("{{\"bindings\": [{binding}]}}")
    }

    fn expect_error(text: &str, message: &str) -> Diagnostic {
        let errors = parse(text).expect_err("expected parse to fail");
        assert_eq!(errors.len(), 1, "expected exactly one error: {errors:?}");
        assert_eq!(errors[0].severity, Severity::Error);
        assert_eq!(errors[0].message, message);
        errors
            .into_iter()
            .next()
            .unwrap_or_else(|| unreachable!("len checked"))
    }

    #[test]
    fn defaults_parse_with_zero_diagnostics() {
        let (bindings, warnings) = parse_with_warnings(DEFAULTS_TEXT).expect("defaults must parse");
        assert!(warnings.is_empty(), "defaults must not warn: {warnings:?}");
        assert_eq!(bindings, defaults());
        assert!(parse(DEFAULTS_TEXT).is_ok());
    }

    #[test]
    fn defaults_match_section_3_table() {
        let bindings = defaults();
        assert_eq!(bindings.bindings.len(), 8);
        let rows = [
            (
                Key::Hold(HoldSide::RightOption),
                true,
                None,
                Command::Dictate,
            ),
            (
                Key::Hold(HoldSide::RightCmd),
                true,
                None,
                Command::VoiceCommand,
            ),
            (
                Key::Chord {
                    modifiers: vec![Modifier::Ctrl, Modifier::Opt],
                    key: "w".into(),
                },
                false,
                None,
                Command::Warm,
            ),
            (
                Key::Chord {
                    modifiers: vec![Modifier::Ctrl, Modifier::Opt],
                    key: "space".into(),
                },
                false,
                None,
                Command::HandsFreeToggle,
            ),
            (
                Key::Chord {
                    modifiers: vec![],
                    key: "escape".into(),
                },
                false,
                Some(When::Listening),
                Command::Cancel,
            ),
            (
                Key::Chord {
                    modifiers: vec![],
                    key: "escape".into(),
                },
                false,
                Some(When::Confirming),
                Command::Cancel,
            ),
            (
                Key::Chord {
                    modifiers: vec![],
                    key: "return".into(),
                },
                false,
                Some(When::Confirming),
                Command::Confirm,
            ),
            (
                Key::Chord {
                    modifiers: vec![Modifier::Ctrl, Modifier::Opt],
                    key: ",".into(),
                },
                false,
                None,
                Command::Settings,
            ),
        ];
        for (index, (key, hold, when, command)) in rows.into_iter().enumerate() {
            assert_eq!(
                bindings.bindings[index],
                Binding {
                    key,
                    hold,
                    when,
                    command
                },
                "default row {index} differs from the §3 table",
            );
        }
    }

    #[test]
    fn reserved_fn() {
        expect_error(
            &single_binding("fn", false, None, "model.warm"),
            "Fn/Globe is reserved for Apple dictation · pick another key",
        );
    }

    #[test]
    fn reserved_globe() {
        expect_error(
            &single_binding("globe", false, None, "model.warm"),
            "Fn/Globe is reserved for Apple dictation · pick another key",
        );
    }

    #[test]
    fn reserved_left_modifier_hold() {
        for key in ["ctrl", "option", "shift", "cmd"] {
            expect_error(
                &single_binding(key, true, None, "voice.dictate"),
                "Hold keys must be a single right-side modifier (right_option, right_cmd, right_shift, right_ctrl)",
            );
        }
    }

    #[test]
    fn reserved_spotlight() {
        expect_error(
            &single_binding("cmd+space", false, None, "model.warm"),
            "cmd+space is a macOS system shortcut (Spotlight) · use another key",
        );
    }

    #[test]
    fn reserved_input_source() {
        expect_error(
            &single_binding("ctrl+space", false, None, "model.warm"),
            "ctrl+space is a macOS system shortcut (input source) · use another key",
        );
    }

    #[test]
    fn reserved_app_switcher() {
        expect_error(
            &single_binding("cmd+tab", false, None, "model.warm"),
            "cmd+tab is a macOS system shortcut (app switcher) · use another key",
        );
    }

    #[test]
    fn reserved_quit() {
        expect_error(
            &single_binding("cmd+q", false, None, "model.warm"),
            "cmd+q quits the frontmost app · use another key",
        );
    }

    #[test]
    fn reserved_hide() {
        expect_error(
            &single_binding("cmd+h", false, None, "model.warm"),
            "cmd+h hides the frontmost app · use another key",
        );
    }

    #[test]
    fn reserved_unscoped_escape() {
        expect_error(
            &single_binding("escape", false, None, "voice.cancel"),
            "escape/return need a \"when\" (listening or confirming) · unscoped escape/return is rejected",
        );
    }

    #[test]
    fn reserved_unscoped_return() {
        expect_error(
            &single_binding("return", false, None, "voice.confirm"),
            "escape/return need a \"when\" (listening or confirming) · unscoped escape/return is rejected",
        );
    }

    #[test]
    fn reserved_plain_letter() {
        expect_error(
            &single_binding("j", false, None, "model.warm"),
            "\"j\" types text · add a modifier or a \"when\"",
        );
    }

    #[test]
    fn reserved_hold_non_modifier() {
        expect_error(
            &single_binding("w", true, None, "voice.dictate"),
            "Only a single right-side modifier can be a hold key",
        );
    }

    #[test]
    fn reserved_hold_chord() {
        expect_error(
            &single_binding("right_option+w", true, None, "voice.dictate"),
            "A hold binding is one key alone · remove the chord or drop \"hold\"",
        );
    }

    #[test]
    fn duplicate_key_when() {
        let text = "{\"bindings\": [\
            {\"key\": \"ctrl+option+w\", \"command\": \"model.warm\"}, \
            {\"key\": \"ctrl+option+w\", \"command\": \"jisuno.settings\"}]}";
        expect_error(
            text,
            "Duplicate binding: \"ctrl+option+w\" (when: global) is already bound to \"model.warm\"",
        );
    }

    #[test]
    fn duplicate_key_when_scoped() {
        let text = "{\"bindings\": [\
            {\"key\": \"escape\", \"when\": \"listening\", \"command\": \"voice.cancel\"}, \
            {\"key\": \"escape\", \"when\": \"listening\", \"command\": \"voice.confirm\"}]}";
        expect_error(
            text,
            "Duplicate binding: \"escape\" (when: listening) is already bound to \"voice.cancel\"",
        );
    }

    #[test]
    fn unknown_command() {
        expect_error(
            &single_binding("ctrl+option+w", false, None, "voice.dance"),
            "Unknown command \"voice.dance\" · see the bindable commands list",
        );
    }

    #[test]
    fn unknown_key_token() {
        expect_error(
            &single_binding("ctrl+hald", false, None, "model.warm"),
            "Unknown key \"hald\" in \"ctrl+hald\"",
        );
    }

    #[test]
    fn wrong_modifier_order() {
        expect_error(
            &single_binding("option+ctrl+w", false, None, "model.warm"),
            "Modifiers must be in ctrl+option+shift+cmd order · got \"option+ctrl+w\"",
        );
    }

    #[test]
    fn uppercase_rejected() {
        expect_error(
            &single_binding("Ctrl+Option+W", false, None, "model.warm"),
            "Keys are lowercase · got \"Ctrl+Option+W\"",
        );
    }

    #[test]
    fn hold_non_right_side_modifier() {
        expect_error(
            &single_binding("ctrl", true, None, "voice.dictate"),
            "Hold keys must be a single right-side modifier (right_option, right_cmd, right_shift, right_ctrl)",
        );
    }

    #[test]
    fn reports_all_errors_not_first() {
        let text = "{\"bindings\": [\
            {\"key\": \"ctrl+hald\", \"command\": \"model.warm\"}, \
            {\"key\": \"ctrl+option+w\", \"command\": \"voice.dance\"}]}";
        let errors = parse(text).expect_err("expected two errors");
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].message, "Unknown key \"hald\" in \"ctrl+hald\"");
        assert_eq!(
            errors[1].message,
            "Unknown command \"voice.dance\" · see the bindable commands list"
        );
    }

    #[test]
    fn canonical_modifier_order_accepted() {
        let text = single_binding("ctrl+option+shift+cmd+f5", false, None, "model.warm");
        let bindings = parse(&text).expect("canonical order must parse");
        assert_eq!(
            bindings.bindings[0].key,
            Key::Chord {
                modifiers: vec![
                    Modifier::Ctrl,
                    Modifier::Opt,
                    Modifier::Shift,
                    Modifier::Cmd
                ],
                key: "f5".into(),
            }
        );
    }

    #[test]
    fn same_key_different_when_is_ok() {
        let text = "{\"bindings\": [\
            {\"key\": \"escape\", \"when\": \"listening\", \"command\": \"voice.cancel\"}, \
            {\"key\": \"escape\", \"when\": \"confirming\", \"command\": \"voice.cancel\"}]}";
        assert!(parse(text).is_ok());
    }

    #[test]
    fn common_shortcut_warnings_all_nine() {
        let cases = [
            ("cmd+w", "Close"),
            ("cmd+s", "Save"),
            ("cmd+z", "Undo"),
            ("cmd+c", "Copy"),
            ("cmd+v", "Paste"),
            ("cmd+x", "Cut"),
            ("cmd+a", "Select All"),
            ("cmd+m", "Minimize"),
            ("cmd+,", "Preferences"),
        ];
        for (key, name) in cases {
            let text = single_binding(key, false, None, "model.warm");
            let (bindings, warnings) =
                parse_with_warnings(&text).expect("warnings must not fail the parse");
            assert_eq!(bindings.bindings.len(), 1);
            assert_eq!(warnings.len(), 1, "expected a warning for {key}");
            assert_eq!(warnings[0].severity, Severity::Warning);
            assert_eq!(
                warnings[0].message,
                format!("Warning: \"{key}\" is a common app shortcut ({name}) · apps will stop getting it")
            );
            assert!(
                parse(&text).is_ok(),
                "warning must not fail parse for {key}"
            );
        }
    }

    #[test]
    fn scoped_cmd_w_does_not_warn() {
        let text = single_binding("cmd+w", false, Some("listening"), "voice.cancel");
        let (_, warnings) = parse_with_warnings(&text).expect("scoped cmd+w must parse");
        assert!(warnings.is_empty());
    }

    #[test]
    fn display_mapping() {
        let cases = [
            ("right_option", "Right ⌥"),
            ("right_cmd", "Right ⌘"),
            ("right_shift", "Right ⇧"),
            ("right_ctrl", "Right ⌃"),
            ("ctrl+option+w", "⌃⌥W"),
            ("ctrl+option+space", "⌃⌥Space"),
            ("escape", "Esc"),
            ("return", "Return"),
            ("tab", "Tab"),
            ("ctrl+option+,", "⌃⌥,"),
            ("ctrl+option+shift+cmd+f5", "⌃⌥⇧⌘F5"),
        ];
        for (file, display) in cases {
            let key = parse_key(file).expect("display case must parse");
            assert_eq!(key.display(), display, "display of {file}");
        }
    }

    #[test]
    fn line_numbers_on_multiline_file() {
        let text = "{\n  \"$schema\": \"./schemas/keybindings.schema.json\",\n  \"bindings\": [\n    {\"key\": \"right_option\", \"hold\": true, \"command\": \"voice.dictate\"},\n    {\"key\": \"ctrl+hald\", \"command\": \"model.warm\"},\n    {\"key\": \"escape\", \"command\": \"voice.cancel\"}\n  ]\n}";
        let errors = parse(text).expect_err("expected errors");
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].line, 5);
        assert_eq!(errors[0].message, "Unknown key \"hald\" in \"ctrl+hald\"");
        assert_eq!(errors[1].line, 6);
        assert!(errors.into_iter().all(|d| d.severity == Severity::Error));
    }

    #[test]
    fn lookup_exact_and_global_fallback() {
        let bindings = defaults();
        let warm = Key::Chord {
            modifiers: vec![Modifier::Ctrl, Modifier::Opt],
            key: "w".into(),
        };
        assert_eq!(bindings.lookup(&warm, None), Some(Command::Warm));
        assert_eq!(
            bindings.lookup(&warm, Some(When::Listening)),
            Some(Command::Warm)
        );

        let esc = Key::Chord {
            modifiers: vec![],
            key: "escape".into(),
        };
        assert_eq!(bindings.lookup(&esc, None), None);
        assert_eq!(
            bindings.lookup(&esc, Some(When::Listening)),
            Some(Command::Cancel)
        );
        assert_eq!(
            bindings.lookup(&esc, Some(When::Confirming)),
            Some(Command::Cancel)
        );

        let missing = Key::Chord {
            modifiers: vec![Modifier::Cmd],
            key: "q".into(),
        };
        assert_eq!(bindings.lookup(&missing, None), None);
    }

    #[test]
    fn conflict_for_settings_recorder() {
        let bindings = defaults();
        let esc = Key::Chord {
            modifiers: vec![],
            key: "escape".into(),
        };
        let clash = bindings
            .conflict(&esc, Some(When::Listening))
            .expect("must clash");
        assert_eq!(clash.command, Command::Cancel);
        assert!(bindings.conflict(&esc, None).is_none());
        let clash = bindings
            .conflict(&esc, Some(When::Confirming))
            .expect("must clash");
        assert_eq!(clash.command, Command::Cancel);
    }

    #[test]
    fn modifier_must_precede_key_name() {
        for key in ["w+ctrl", "ctrl+w+option", "option+ctrl+w", "ctrl+ctrl+w"] {
            expect_error(
                &single_binding(key, false, None, "model.warm"),
                &format!("Modifiers must be in ctrl+option+shift+cmd order · got \"{key}\""),
            );
        }
    }

    #[test]
    fn unknown_top_level_field() {
        expect_error(
            "{\"$schema\": \"./schemas/keybindings.schema.json\", \"bindings\": [], \"theme\": \"dark\"}",
            "Unknown field \"theme\"",
        );
    }

    #[test]
    fn missing_bindings_is_error() {
        let errors = parse("{}").expect_err("missing bindings must fail");
        assert!(
            errors.iter().any(|d| d.is_error()),
            "unexpected diagnostics: {errors:?}"
        );
    }

    #[test]
    fn unknown_binding_field_reports_binding_line() {
        let text = concat!(
            "{\n",
            "  \"bindings\": [\n",
            "    {\"key\": \"ctrl+option+w\", \"command\": \"model.warm\", \"repeat\": true}\n",
            "  ]\n",
            "}",
        );
        let errors = parse(text).expect_err("unknown field must fail");
        assert_eq!(errors.len(), 1, "unexpected errors: {errors:?}");
        assert_eq!(errors[0].line, 3);
        assert_eq!(errors[0].message, "Unknown field \"repeat\" in binding");
    }

    #[test]
    fn binding_type_errors_report_binding_line() {
        let text = concat!(
            "{\n",
            "  \"bindings\": [\n",
            "    {\"key\": \"ctrl+option+w\", \"hold\": \"true\", \"command\": \"model.warm\"},\n",
            "    {\"key\": 5, \"command\": \"model.warm\"},\n",
            "    {\"key\": \"ctrl+hald\", \"command\": \"model.warm\"}\n",
            "  ]\n",
            "}",
        );
        let errors = parse(text).expect_err("type errors must fail");
        assert_eq!(errors.len(), 3, "unexpected errors: {errors:?}");
        assert_eq!(errors[0].line, 3);
        assert_eq!(errors[0].message, "\"hold\" must be a boolean");
        assert_eq!(errors[1].line, 4);
        assert_eq!(errors[1].message, "\"key\" must be a string");
        assert_eq!(errors[2].line, 5);
        assert_eq!(errors[2].message, "Unknown key \"hald\" in \"ctrl+hald\"");
    }

    #[test]
    fn right_side_key_needs_hold() {
        for key in ["right_option", "right_cmd", "right_shift", "right_ctrl"] {
            expect_error(
                &single_binding(key, false, None, "voice.dictate"),
                &format!("\"{key}\" is a hold key · set \"hold\": true"),
            );
        }
    }

    #[test]
    fn function_key_range() {
        for key in ["f1", "f20"] {
            parse(&single_binding(key, false, None, "model.warm"))
                .unwrap_or_else(|e| panic!("{key} must validate: {e:?}"));
        }
        for key in ["f0", "f05", "f21"] {
            expect_error(
                &single_binding(key, false, None, "model.warm"),
                &format!("Unknown key \"{key}\" in \"{key}\""),
            );
        }
    }

    #[test]
    fn invalid_when_reports_once() {
        // "j" would also be a typing-key error with no `when`; an invalid
        // `when` must not add the reserved-key error.
        expect_error(
            &single_binding("j", false, Some("sleeping"), "model.warm"),
            "Unknown when \"sleeping\" · use listening or confirming",
        );
    }

    #[test]
    fn key_parse_is_canonical_and_order_independent() {
        let parsed = Key::parse("ctrl+option+w").expect("canonical chord must parse");
        assert_eq!(parsed.to_file_string(), "ctrl+option+w");
        let ordered = Key::Chord {
            modifiers: vec![Modifier::Ctrl, Modifier::Opt],
            key: "w".into(),
        };
        let swapped = Key::Chord {
            modifiers: vec![Modifier::Opt, Modifier::Ctrl],
            key: "w".into(),
        };
        assert_eq!(parsed, ordered);
        assert_eq!(swapped, ordered);
        assert!(Key::parse("option+ctrl+w").is_err());
        assert_eq!(
            Key::parse("right_option")
                .expect("hold must parse")
                .to_file_string(),
            "right_option"
        );
    }

    #[test]
    fn settings_recorder_api() {
        assert_eq!(When::Listening.as_str(), "listening");
        assert_eq!(When::Confirming.as_str(), "confirming");
        assert_eq!(Command::Dictate.as_str(), "voice.dictate");
        assert_eq!(Command::Dictate.id(), Command::Dictate.as_str());

        let bindings = defaults();
        let escape = Key::parse("escape").expect("escape must parse");
        let clash = bindings
            .conflict(&escape, Some(When::Listening))
            .expect("escape is taken in listening");
        let rendered = format!(
            "Taken by {} ({})",
            clash.command.as_str(),
            clash.when.map(When::as_str).unwrap_or("global")
        );
        assert_eq!(rendered, "Taken by voice.cancel (listening)");
    }

    #[test]
    fn validate_single_binding() {
        let ok = Binding {
            key: Key::parse("ctrl+option+w").expect("must parse"),
            hold: false,
            when: None,
            command: Command::Warm,
        };
        assert!(validate_binding(&ok).is_empty());

        let hold = Binding {
            key: Key::parse("right_option").expect("must parse"),
            hold: true,
            when: None,
            command: Command::Dictate,
        };
        assert!(validate_binding(&hold).is_empty());

        let missing_hold = Binding {
            key: Key::parse("right_option").expect("must parse"),
            hold: false,
            when: None,
            command: Command::Dictate,
        };
        let diagnostics = validate_binding(&missing_hold);
        assert_eq!(diagnostics.len(), 1, "unexpected: {diagnostics:?}");
        assert_eq!(diagnostics[0].line, 0);
        assert_eq!(
            diagnostics[0].message,
            "\"right_option\" is a hold key · set \"hold\": true"
        );

        let bare_escape = Binding {
            key: Key::parse("escape").expect("must parse"),
            hold: false,
            when: None,
            command: Command::Cancel,
        };
        let diagnostics = validate_binding(&bare_escape);
        assert_eq!(diagnostics.len(), 1, "unexpected: {diagnostics:?}");
        assert_eq!(diagnostics[0].line, 0);
        assert!(diagnostics[0].is_error());
    }
}
