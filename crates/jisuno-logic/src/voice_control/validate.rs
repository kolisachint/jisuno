//! Validation: the §7 checks, plus typed conversion.
//!
//! Checks run over `serde_json::Value` (not the typed [`Config`](super::Config))
//! so unknown keys, unknown actions, and missing arguments produce the §7
//! messages instead of a generic deserialization error. [`convert`] builds the
//! typed config and runs only when no errors were found.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::Value;

use super::scan::LineMap;
use super::{
    is_slot_token, normalize, CommandDef, Config, Diagnostic, Place, PlaceOrSlot, SlotDecl, Step,
    WhenApp, WindowDef,
};

const KNOWN_TOP_KEYS: &[&str] = &["$schema", "version", "commands", "workspaces"];
const KNOWN_COMMAND_KEYS: &[&str] = &["id", "say", "steps", "confirm", "when", "slots", "agent"];
const KNOWN_WHEN_KEYS: &[&str] = &["app"];
const KNOWN_WINDOW_KEYS: &[&str] = &["id", "app", "title", "open", "run", "place"];
const BUILTIN_LIST_SLOTS: &[&str] = &["window", "place", "workspace", "theme", "app"];
const BUILTIN_FREE_SLOT: &str = "name";

/// (required args, optional args) per action. `wait` is validated by checks
/// 18–19; its lists only feed check 11.
fn action_spec(action: &str) -> Option<(&'static [&'static str], &'static [&'static str])> {
    match action {
        "jisuno.menu" => Some((&[], &[])),
        "jisuno.settings" => Some((&[], &[])),
        "app.open" => Some((&["app"], &["arg"])),
        "window.focus" => Some((&["window"], &[])),
        "window.tile" => Some((&["window", "place"], &[])),
        "workspace.open" => Some((&["workspace"], &[])),
        "workspace.close" => Some((&["workspace"], &[])),
        "ax.press" => Some((&["app", "role", "name"], &[])),
        "ax.set_value" => Some((&["app", "role", "name", "value"], &[])),
        "keys.send" => Some((&["keys"], &[])),
        "shell.run" => Some((&["cmd"], &["cwd"])),
        "theme.set" => Some((&["theme"], &[])),
        "model.warm" => Some((&[], &[])),
        "model.unload" => Some((&[], &[])),
        "wait" => Some((&[], &["window", "ms"])),
        _ => None,
    }
}

/// `command "<id>"` when the id is known, else `command <n>`.
fn addr(id: Option<&str>, index: usize) -> String {
    match id {
        Some(text) => format!("\"{text}\""),
        None => format!("{}", index + 1),
    }
}

fn command_line(map: &LineMap, index: usize) -> usize {
    map.commands.get(index).copied().unwrap_or(1)
}

fn step_line(map: &LineMap, ci: usize, si: usize, fallback: usize) -> usize {
    map.steps
        .get(ci)
        .and_then(|steps| steps.get(si))
        .copied()
        .unwrap_or(fallback)
}

/// Compact rendering of a JSON value for messages (strings without quotes;
/// the message template adds them).
fn render_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        _ => serde_json::to_string(value).unwrap_or_else(|_| "?".to_string()),
    }
}

/// Bundle id per the schema pattern `^[A-Za-z][A-Za-z0-9-]*(\.[A-Za-z0-9-]+)+$`.
fn is_bundle_id(text: &str) -> bool {
    let mut parts = text.split('.');
    match parts.next() {
        Some(first) if is_bid_first(first) => {}
        _ => return false,
    }
    let mut count = 1;
    for part in parts {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return false;
        }
        count += 1;
    }
    count >= 2
}

fn is_bid_first(part: &str) -> bool {
    let mut bytes = part.bytes();
    match bytes.next() {
        Some(b'A'..=b'Z' | b'a'..=b'z') => {}
        _ => return false,
    }
    bytes.all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// `[a-z_][a-z0-9_]*`: the only slot names (§5). The runner fills exactly
/// these, so anything else inside braces is not a slot.
fn is_slot_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first == '_' || first.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Identifier-shaped text that breaks the slot-name rule (e.g. `Window`).
fn is_ident_like(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first == '_' || first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Brace references in `text`: `(slots, bad)`. A `{name}` with a valid
/// slot name is a slot; an identifier-shaped `{name}` that breaks the rule
/// (uppercase, etc.) is `bad`. `${...}` is never a reference (shell vars),
/// and other brace text (`{src,test}`) is ignored.
fn brace_refs(text: &str) -> (Vec<String>, Vec<String>) {
    let bytes = text.as_bytes();
    let mut slots = Vec::new();
    let mut bad = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' && (i == 0 || bytes[i - 1] != b'$') {
            let run = bytes[i + 1..]
                .iter()
                .take_while(|b| b.is_ascii_alphanumeric() || **b == b'_')
                .count();
            let close = i + 1 + run;
            if run > 0 && close < bytes.len() && bytes[close] == b'}' {
                let name = &text[i + 1..close];
                if is_slot_name(name) {
                    slots.push(name.to_string());
                } else if is_ident_like(name) {
                    bad.push(name.to_string());
                }
                i = close + 1;
                continue;
            }
        }
        i += 1;
    }
    (slots, bad)
}

/// Slot names in normalized phrase words (`{slot}` tokens, §5).
fn phrase_slots(words: &[String]) -> Vec<String> {
    words
        .iter()
        .filter(|word| is_slot_token(word))
        .map(|word| word[1..word.len() - 1].to_string())
        .collect()
}

/// Char-level Levenshtein distance.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, &ca) in a.iter().enumerate() {
        let mut next = vec![i + 1; b.len() + 1];
        for (j, &cb) in b.iter().enumerate() {
            next[j + 1] = (prev[j] + usize::from(ca != cb))
                .min(prev[j + 1] + 1)
                .min(next[j] + 1);
        }
        prev = next;
    }
    prev[b.len()]
}

/// A declared slot after its declaration is read. `Bad` is reported once by
/// check 25; phrases then treat it as declared so the error is not repeated.
#[derive(Debug, Clone, PartialEq)]
enum Decl {
    List(Vec<String>),
    Free,
    Bad,
}

/// Built-in slot names (§5). Declared slots cannot reuse them (check 22).
fn is_builtin_slot(name: &str) -> bool {
    BUILTIN_LIST_SLOTS.contains(&name) || name == BUILTIN_FREE_SLOT
}

/// `{name}` when the whole text is one brace reference (a `place` value).
fn whole_slot(text: &str) -> Option<&str> {
    let inner = text.strip_prefix('{')?.strip_suffix('}')?;
    (!inner.is_empty() && inner.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        .then_some(inner)
}

/// A list value naming a place, in wire or spoken form (`right half`).
fn place_value(value: &str) -> Option<Place> {
    Place::from_str(&normalize(value).join("-"))
}

/// Per-command facts gathered for the cross-command checks (7, 21).
struct CmdFacts {
    id: Option<String>,
    line: usize,
    phrases: Vec<(String, Vec<String>)>,
    when: Option<HashSet<String>>,
}

/// Run the §7 checks. Returns `(errors, warnings)`, each sorted by
/// `(line, message)` for deterministic output.
pub fn validate(root: &Value, map: &LineMap) -> (Vec<Diagnostic>, Vec<Diagnostic>) {
    let mut errors: Vec<Diagnostic> = Vec::new();
    let mut warnings: Vec<Diagnostic> = Vec::new();
    let error = |line: usize, message: String| Diagnostic {
        line,
        severity: super::Severity::Error,
        message,
    };

    let Some(obj) = root.as_object() else {
        return (
            vec![error(1, "config must be an object".to_string())],
            Vec::new(),
        );
    };

    // Check 2 (file level).
    for key in obj.keys() {
        if !KNOWN_TOP_KEYS.contains(&key.as_str()) {
            errors.push(error(1, format!("unknown key \"{key}\"")));
        }
    }

    // Version (schema `const: 1`; no §7 row, rejected outright).
    match obj.get("version") {
        Some(Value::Number(n)) if n.as_u64() == Some(1) => {}
        Some(other) => errors.push(error(
            1,
            format!("unsupported \"version\": {}", render_value(other)),
        )),
        None => errors.push(error(1, "missing \"version\"".to_string())),
    }

    // Workspaces: collected first so check 16 can name known workspaces.
    let mut workspace_names: HashSet<String> = HashSet::new();
    match obj.get("workspaces") {
        None => {}
        Some(Value::Object(workspaces)) => {
            for (name, windows) in workspaces {
                workspace_names.insert(name.clone());
                let lines = map.windows.get(name);
                let Some(list) = windows.as_array() else {
                    errors.push(error(1, format!("workspace \"{name}\" must be an array")));
                    continue;
                };
                for (k, window) in list.iter().enumerate() {
                    let line = lines.and_then(|l| l.get(k)).copied().unwrap_or(1);
                    let waddr = match window.get("id").and_then(Value::as_str) {
                        Some(id) if !id.is_empty() => format!("\"{id}\""),
                        _ => format!("{}", k + 1),
                    };
                    let Some(wobj) = window.as_object() else {
                        errors.push(error(
                            line,
                            format!("workspace \"{name}\" window {} must be an object", k + 1),
                        ));
                        continue;
                    };
                    for key in wobj.keys() {
                        if !KNOWN_WINDOW_KEYS.contains(&key.as_str()) {
                            errors.push(error(
                                line,
                                format!(
                                    "unknown key \"{key}\" in workspace \"{name}\" window {waddr}"
                                ),
                            ));
                        }
                    }
                    for field in ["id", "app", "place"] {
                        match wobj.get(field).and_then(Value::as_str) {
                            Some(text) if !text.is_empty() => {}
                            _ => errors.push(error(
                                line,
                                format!("workspace \"{name}\" window {waddr}: missing \"{field}\""),
                            )),
                        }
                    }
                    if let Some(Value::String(place)) = wobj.get("place") {
                        if super::Place::from_str(place).is_none() {
                            errors.push(error(
                                line,
                                format!(
                                    "workspace \"{name}\" window {waddr}: bad place \"{place}\""
                                ),
                            ));
                        }
                    }
                    if wobj.contains_key("open") && wobj.contains_key("run") {
                        errors.push(error(
                            line,
                            format!("workspace \"{name}\" window {waddr}: only one of \"open\" or \"run\""),
                        ));
                    }
                    for field in ["title", "open", "run"] {
                        if let Some(value) = wobj.get(field) {
                            let bad = match value {
                                Value::String(text) => (field != "title") && text.is_empty(),
                                _ => true,
                            };
                            if bad {
                                errors.push(error(
                                    line,
                                    format!("workspace \"{name}\" window {waddr}: bad \"{field}\""),
                                ));
                            }
                        }
                    }
                }
            }
        }
        Some(_) => errors.push(error(1, "\"workspaces\" must be an object".to_string())),
    }

    // Commands.
    let mut facts: Vec<CmdFacts> = Vec::new();
    let commands = match obj.get("commands") {
        Some(Value::Array(commands)) => Some(commands),
        _ => {
            errors.push(error(1, "missing \"commands\"".to_string()));
            None
        }
    };
    if let Some(commands) = commands {
        for (i, command) in commands.iter().enumerate() {
            let line = command_line(map, i);
            let Some(cobj) = command.as_object() else {
                errors.push(error(line, format!("command {}: missing \"id\"", i + 1)));
                continue;
            };
            let id = match cobj.get("id").and_then(Value::as_str) {
                Some(text) if !text.is_empty() => Some(text.to_string()),
                _ => {
                    errors.push(error(line, format!("command {}: missing \"id\"", i + 1)));
                    None
                }
            };
            let tag = addr(id.as_deref(), i);

            // Check 2 (command level).
            for key in cobj.keys() {
                if !KNOWN_COMMAND_KEYS.contains(&key.as_str()) {
                    errors.push(error(
                        line,
                        format!("unknown key \"{key}\" in command {tag}"),
                    ));
                }
            }

            // Checks 4–5.
            let say = match cobj.get("say") {
                Some(Value::Array(say)) if !say.is_empty() => Some(say),
                _ => {
                    errors.push(error(line, format!("command {tag}: missing \"say\"")));
                    None
                }
            };
            let steps = match cobj.get("steps") {
                Some(Value::Array(steps)) if !steps.is_empty() => Some(steps),
                _ => {
                    errors.push(error(line, format!("command {tag}: missing \"steps\"")));
                    None
                }
            };

            // Check 8 + phrase inventory.
            let mut phrases: Vec<(String, Vec<String>)> = Vec::new();
            if let Some(say) = say {
                for item in say {
                    match item.as_str() {
                        Some(phrase) => {
                            let words = normalize(phrase);
                            if words.is_empty() {
                                errors.push(error(
                                    line,
                                    format!("command {tag}: empty phrase in \"say\""),
                                ));
                            } else {
                                for name in brace_refs(phrase).1 {
                                    errors.push(error(
                                        line,
                                        format!("command {tag}: bad slot name \"{name}\""),
                                    ));
                                }
                                phrases.push((phrase.to_string(), words));
                            }
                        }
                        None => errors.push(error(
                            line,
                            format!("command {tag}: empty phrase in \"say\""),
                        )),
                    }
                }
            }

            // Step checks 9–11, 16–19.
            let mut has_shell = false;
            let mut step_refs: HashSet<String> = HashSet::new();
            let mut place_refs: Vec<(String, usize)> = Vec::new();
            if let Some(steps) = steps {
                for (j, step) in steps.iter().enumerate() {
                    let n = j + 1;
                    let sline = step_line(map, i, j, line);
                    let Some(sobj) = step.as_object() else {
                        errors.push(error(sline, format!("command {tag}: unknown action \"\"")));
                        continue;
                    };
                    let do_raw = sobj.get("do");
                    let action = do_raw.and_then(Value::as_str).unwrap_or("");
                    let Some((required, optional)) = action_spec(action) else {
                        errors.push(error(
                            sline,
                            format!(
                                "command {tag}: unknown action \"{}\"",
                                do_raw.map(render_value).unwrap_or_default()
                            ),
                        ));
                        continue;
                    };
                    let known: HashSet<&str> = required
                        .iter()
                        .chain(optional.iter())
                        .chain(["do"].iter())
                        .copied()
                        .collect();
                    // Check 11.
                    let mut unknown: Vec<&String> = sobj
                        .keys()
                        .filter(|k| !known.contains(k.as_str()))
                        .collect();
                    unknown.sort();
                    for key in unknown {
                        errors.push(error(
                            sline,
                            format!(
                                "command {tag} step {n} ({action}): unknown argument \"{key}\""
                            ),
                        ));
                    }
                    if action == "shell.run" {
                        has_shell = true;
                    }
                    if action == "wait" {
                        // Checks 18–19.
                        let has_window = sobj.contains_key("window");
                        let has_ms = sobj.contains_key("ms");
                        if has_window == has_ms {
                            errors.push(error(
                                sline,
                                format!(
                                    "command {tag} step {n} (wait): exactly one of \"window\" or \"ms\""
                                ),
                            ));
                        }
                        if let Some(ms) = sobj.get("ms") {
                            let ok = ms.as_u64().is_some_and(|v| (1..=10000).contains(&v));
                            if !ok {
                                errors.push(error(
                                    sline,
                                    format!(
                                        "command {tag} step {n} (wait): \"ms\" must be <= 10000"
                                    ),
                                ));
                            }
                        }
                        if let Some(window) = sobj.get("window") {
                            if window.as_str().is_none_or(|w| w.is_empty()) {
                                errors.push(error(
                                    sline,
                                    format!(
                                        "command {tag} step {n} (wait): missing argument \"window\""
                                    ),
                                ));
                            }
                        }
                        continue;
                    }
                    // Check 10.
                    for arg in required {
                        match sobj.get(*arg) {
                            Some(Value::String(text)) if !text.is_empty() => {}
                            _ => errors.push(error(
                                sline,
                                format!(
                                    "command {tag} step {n} ({action}): missing argument \"{arg}\""
                                ),
                            )),
                        }
                    }
                    // Checks 16–17 (literals only; `{slot}` refs resolve at runtime).
                    if action == "workspace.open" || action == "workspace.close" {
                        if let Some(Value::String(target)) = sobj.get("workspace") {
                            if !is_slot_token(&target.to_lowercase())
                                && !workspace_names.contains(target)
                            {
                                errors.push(error(
                                    sline,
                                    format!("command {tag}: unknown workspace \"{target}\""),
                                ));
                            }
                        } else if let Some(other) = sobj.get("workspace") {
                            errors.push(error(
                                sline,
                                format!(
                                    "command {tag}: unknown workspace \"{}\"",
                                    render_value(other)
                                ),
                            ));
                        }
                    }
                    if action == "window.tile" {
                        if let Some(Value::String(place)) = sobj.get("place") {
                            if let Some(name) = whole_slot(place) {
                                place_refs.push((name.to_string(), sline));
                            } else if super::Place::from_str(place).is_none() {
                                errors.push(error(
                                    sline,
                                    format!("command {tag}: bad place \"{place}\""),
                                ));
                            }
                        } else if let Some(other) = sobj.get("place") {
                            errors.push(error(
                                sline,
                                format!("command {tag}: bad place \"{}\"", render_value(other)),
                            ));
                        }
                    }
                    for (key, value) in sobj.iter() {
                        if key != "do" {
                            if let Some(text) = value.as_str() {
                                let (slots, bad) = brace_refs(text);
                                for name in bad {
                                    errors.push(error(
                                        sline,
                                        format!("command {tag}: bad slot name \"{name}\""),
                                    ));
                                }
                                step_refs.extend(slots);
                            }
                        }
                    }
                }
            }

            // `when` (check 2 + check 20).
            let mut when: Option<HashSet<String>> = None;
            match cobj.get("when") {
                None => {}
                Some(Value::Object(when_obj)) => {
                    for key in when_obj.keys() {
                        if !KNOWN_WHEN_KEYS.contains(&key.as_str()) {
                            errors.push(error(
                                line,
                                format!("unknown key \"{key}\" in command {tag} when"),
                            ));
                        }
                    }
                    match when_obj.get("app") {
                        Some(Value::Array(apps)) if !apps.is_empty() => {
                            let mut set = HashSet::new();
                            for app in apps {
                                match app.as_str() {
                                    Some(text) if is_bundle_id(text) => {
                                        set.insert(text.to_string());
                                    }
                                    _ => errors.push(error(
                                        line,
                                        format!(
                                            "command {tag}: bad bundle id \"{}\"",
                                            render_value(app)
                                        ),
                                    )),
                                }
                            }
                            when = Some(set);
                        }
                        _ => {
                            errors.push(error(line, format!("command {tag}: missing \"when.app\"")))
                        }
                    }
                }
                Some(_) => errors.push(error(line, format!("command {tag}: bad \"when\""))),
            }

            // Declared slots.
            let mut decls: BTreeMap<String, Decl> = BTreeMap::new();
            match cobj.get("slots") {
                None => {}
                Some(Value::Object(slots)) => {
                    for (name, value) in slots {
                        let decl = match value {
                            Value::String(marker) if marker == "free" => Decl::Free,
                            Value::Array(items)
                                if !items.is_empty()
                                    && items
                                        .iter()
                                        .all(|v| v.as_str().is_some_and(|s| !s.is_empty())) =>
                            {
                                Decl::List(
                                    items
                                        .iter()
                                        .filter_map(Value::as_str)
                                        .map(str::to_string)
                                        .collect(),
                                )
                            }
                            _ => Decl::Bad,
                        };
                        decls.insert(name.to_lowercase(), decl);
                    }
                }
                Some(_) => errors.push(error(line, format!("command {tag}: bad \"slots\""))),
            }
            let known_slot = |name: &str| match decls.get(name) {
                Some(Decl::List(_)) | Some(Decl::Free) => true,
                Some(Decl::Bad) => false,
                None => BUILTIN_LIST_SLOTS.contains(&name) || name == BUILTIN_FREE_SLOT,
            };
            let free_slot = |name: &str| match decls.get(name) {
                Some(Decl::Free) => true,
                Some(_) => false,
                None => name == BUILTIN_FREE_SLOT,
            };

            // Check 13.
            for (raw, words) in &phrases {
                let mut slots = phrase_slots(words);
                slots.sort();
                slots.dedup();
                for slot in slots {
                    if !known_slot(&slot) {
                        errors.push(error(
                            line,
                            format!(
                                "command {tag}: slot \"{{{slot}}}\" in phrase \"{raw}\" has no list or declaration"
                            ),
                        ));
                    }
                }
            }

            // Check 14.
            for (raw, _) in &phrases {
                let tokens: Vec<&str> = raw.split_whitespace().collect();
                let mut free_at: Vec<usize> = Vec::new();
                for (t, token) in tokens.iter().enumerate() {
                    let lower = token.to_lowercase();
                    if is_slot_token(&lower) && free_slot(&lower[1..lower.len() - 1]) {
                        free_at.push(t);
                    }
                }
                if free_at.len() > 1 || free_at.first().is_some_and(|&t| t + 1 != tokens.len()) {
                    errors.push(error(
                        line,
                        format!(
                            "command {tag}: free slot must be the only free slot and the last token: \"{raw}\""
                        ),
                    ));
                }
            }

            // Check 12.
            let phrase_slot_set: HashSet<String> = phrases
                .iter()
                .flat_map(|(_, words)| phrase_slots(words))
                .collect();
            let mut refs: Vec<String> = step_refs.into_iter().collect();
            refs.sort();
            for slot in refs {
                if !phrase_slot_set.contains(&slot) {
                    errors.push(error(
                        line,
                        format!(
                            "command {tag}: slot \"{{{slot}}}\" used in steps but not in any phrase"
                        ),
                    ));
                }
            }

            // Check 15.
            let confirmed = cobj
                .get("confirm")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if has_shell && !confirmed {
                errors.push(error(
                    line,
                    format!("command {tag}: shell.run requires \"confirm\": true"),
                ));
            }

            facts.push(CmdFacts {
                id,
                line,
                phrases,
                when,
            });
        }
    }

    // Check 6.
    {
        let mut seen: HashMap<String, usize> = HashMap::new();
        for fact in facts.iter() {
            if let Some(id) = &fact.id {
                if seen.contains_key(id) {
                    errors.push(error(fact.line, format!("duplicate id \"{id}\"")));
                } else {
                    seen.insert(id.clone(), fact.line);
                }
            }
        }
    }

    // Check 7.
    {
        let usable =
            |when: &Option<HashSet<String>>| when.as_ref().filter(|set| !set.is_empty()).cloned();
        let mut seen: HashMap<String, (String, Option<HashSet<String>>)> = HashMap::new();
        for fact in &facts {
            let Some(id) = &fact.id else { continue };
            let current = usable(&fact.when);
            for (_, words) in &fact.phrases {
                let key = words.join(" ");
                match seen.get(&key) {
                    Some((first_id, first_when)) => {
                        let exempt = match (usable(first_when), current.clone()) {
                            (Some(a), Some(b)) => a.is_disjoint(&b),
                            _ => false,
                        };
                        if !exempt {
                            errors.push(error(
                                fact.line,
                                format!(
                                    "duplicate phrase \"{key}\" (commands \"{first_id}\" and \"{id}\")"
                                ),
                            ));
                        }
                    }
                    None => {
                        seen.insert(key, (id.clone(), current.clone()));
                    }
                }
            }
        }
    }

    // Check 21 (warning). §5 ties break within ~5% of the best score; this
    // check approximates that as: same word count, exactly one differing
    // word, and the two words within char-level edit distance 2 (e.g. "open
    // setting" vs "open settings"). Pairs scoped to disjoint `when.app`
    // lists cannot tie (filtering in §5 removes one before matching), so
    // they are exempt like in check 7.
    {
        let usable =
            |when: &Option<HashSet<String>>| when.as_ref().filter(|set| !set.is_empty()).cloned();
        for (j, later) in facts.iter().enumerate() {
            let Some(later_id) = &later.id else { continue };
            for earlier in facts.iter().take(j) {
                let Some(earlier_id) = &earlier.id else {
                    continue;
                };
                if earlier_id == later_id && earlier.line == later.line {
                    continue;
                }
                let exempt = match (usable(&earlier.when), usable(&later.when)) {
                    (Some(a), Some(b)) => a.is_disjoint(&b),
                    _ => false,
                };
                if exempt {
                    continue;
                }
                for (later_raw, later_words) in &later.phrases {
                    for (earlier_raw, earlier_words) in &earlier.phrases {
                        if later_words.len() != earlier_words.len() {
                            continue;
                        }
                        let diffs: Vec<usize> = later_words
                            .iter()
                            .zip(earlier_words.iter())
                            .enumerate()
                            .filter(|(_, (a, b))| a != b)
                            .map(|(k, _)| k)
                            .collect();
                        if diffs.len() != 1 {
                            continue;
                        }
                        let k = diffs[0];
                        let distance = edit_distance(&later_words[k], &earlier_words[k]);
                        if (1..=2).contains(&distance) {
                            warnings.push(Diagnostic::warning(
                                later.line,
                                format!(
                                    "phrase \"{later_raw}\" is very close to \"{earlier_raw}\" (commands \"{later_id}\" and \"{earlier_id}\")"
                                ),
                            ));
                        }
                    }
                }
            }
        }
    }

    errors.sort_by(|a, b| (a.line, &a.message).cmp(&(b.line, &b.message)));
    warnings.sort_by(|a, b| (a.line, &a.message).cmp(&(b.line, &b.message)));
    (errors, warnings)
}

/// Build the typed config. Runs only when [`validate`] found no errors, so
/// every access below is guarded by a check above; fallbacks are unreachable.
pub fn convert(root: &Value) -> Config {
    let version = root.get("version").and_then(Value::as_u64).unwrap_or(1) as u32;
    let mut commands = Vec::new();
    if let Some(Value::Array(list)) = root.get("commands") {
        for command in list {
            if let Some(built) = convert_command(command) {
                commands.push(built);
            }
        }
    }
    let mut workspaces: BTreeMap<String, Vec<WindowDef>> = BTreeMap::new();
    if let Some(Value::Object(map)) = root.get("workspaces") {
        for (name, windows) in map {
            if let Some(list) = windows.as_array() {
                workspaces.insert(
                    name.clone(),
                    list.iter().filter_map(convert_window).collect(),
                );
            }
        }
    }
    Config {
        schema: root
            .get("$schema")
            .and_then(Value::as_str)
            .map(str::to_string),
        version,
        commands,
        workspaces,
    }
}

fn convert_command(command: &Value) -> Option<CommandDef> {
    let obj = command.as_object()?;
    let id = obj
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let say = obj
        .get("say")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let steps = obj
        .get("steps")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(convert_step).collect())
        .unwrap_or_default();
    let confirm = obj.get("confirm").and_then(Value::as_bool).unwrap_or(false);
    let agent = obj.get("agent").and_then(Value::as_bool).unwrap_or(false);
    let when = match obj.get("when") {
        Some(Value::Object(when_obj)) => {
            when_obj
                .get("app")
                .and_then(Value::as_array)
                .map(|apps| WhenApp {
                    app: apps
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect(),
                })
        }
        _ => None,
    };
    let mut slots = BTreeMap::new();
    if let Some(Value::Object(decls)) = obj.get("slots") {
        for (name, value) in decls {
            match value {
                Value::String(marker) if marker == "free" => {
                    slots.insert(name.clone(), SlotDecl::Free);
                }
                Value::Array(items) => {
                    slots.insert(
                        name.clone(),
                        SlotDecl::List(
                            items
                                .iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect(),
                        ),
                    );
                }
                _ => {}
            }
        }
    }
    Some(CommandDef {
        id,
        say,
        steps,
        confirm,
        when,
        slots,
        agent,
    })
}

fn convert_step(step: &Value) -> Option<Step> {
    let obj = step.as_object()?;
    let action = obj.get("do").and_then(Value::as_str).unwrap_or("");
    let string = |key: &str| {
        obj.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let opt_string = |key: &str| obj.get(key).and_then(Value::as_str).map(str::to_string);
    match action {
        "jisuno.menu" => Some(Step::JisunoMenu),
        "jisuno.settings" => Some(Step::JisunoSettings),
        "app.open" => Some(Step::AppOpen {
            app: string("app"),
            arg: opt_string("arg"),
        }),
        "window.focus" => Some(Step::WindowFocus {
            window: string("window"),
        }),
        "window.tile" => {
            let place = match obj.get("place").and_then(Value::as_str) {
                Some(text) => match Place::from_str(text) {
                    Some(place) => PlaceOrSlot::Place(place),
                    None => PlaceOrSlot::Slot(text.to_string()),
                },
                None => PlaceOrSlot::Slot(String::new()),
            };
            Some(Step::WindowTile {
                window: string("window"),
                place,
            })
        }
        "workspace.open" => Some(Step::WorkspaceOpen {
            workspace: string("workspace"),
        }),
        "workspace.close" => Some(Step::WorkspaceClose {
            workspace: string("workspace"),
        }),
        "ax.press" => Some(Step::AxPress {
            app: string("app"),
            role: string("role"),
            name: string("name"),
        }),
        "ax.set_value" => Some(Step::AxSetValue {
            app: string("app"),
            role: string("role"),
            name: string("name"),
            value: string("value"),
        }),
        "keys.send" => Some(Step::KeysSend {
            keys: string("keys"),
        }),
        "shell.run" => Some(Step::ShellRun {
            cmd: string("cmd"),
            cwd: opt_string("cwd"),
        }),
        "theme.set" => Some(Step::ThemeSet {
            theme: string("theme"),
        }),
        "model.warm" => Some(Step::ModelWarm),
        "model.unload" => Some(Step::ModelUnload),
        "wait" => Some(Step::Wait {
            window: opt_string("window"),
            ms: obj.get("ms").and_then(Value::as_u64),
        }),
        _ => None,
    }
}

fn convert_window(window: &Value) -> Option<WindowDef> {
    let obj = window.as_object()?;
    let string = |key: &str| {
        obj.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let opt_string = |key: &str| obj.get(key).and_then(Value::as_str).map(str::to_string);
    Some(WindowDef {
        id: string("id"),
        app: string("app"),
        title: opt_string("title"),
        open: opt_string("open"),
        run: opt_string("run"),
        // Unreachable after validation: `place` is a known value.
        place: obj
            .get("place")
            .and_then(Value::as_str)
            .and_then(Place::from_str)
            .unwrap_or(Place::Full),
    })
}
