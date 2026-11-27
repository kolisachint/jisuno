//! `settings.json`: typed model, parse with line-numbered diagnostics, the
//! first-run config writer, atomic writes, and the last-good holder for hot reload.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{env, fmt};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::embedded;

const DICTATION_LIMIT_S: RangeInclusive<u32> = 10..=600;
const HANDS_FREE_SILENCE_MS: RangeInclusive<u32> = 200..=5000;

/// Bounds the symlink chain followed for a dangling link, as the kernel does.
const MAX_SYMLINK_HOPS: usize = 40;

/// Makes each temp file name unique within the process. Paired with the pid across processes.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Top-level keys of settings.json, as in `schemas/settings.schema.json`.
const KNOWN_KEYS: &[&str] = &[
    "$schema",
    "model",
    "model_load",
    "unload_after_idle_min",
    "input_device",
    "dictation_limit_s",
    "hands_free_silence_ms",
    "partials",
    "insert",
    "insert_type_apps",
    "restore_clipboard",
    "sounds",
    "hud",
    "login_item",
];

/// The `settings.json` document. Missing keys take their default; unknown keys are errors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    #[serde(rename = "$schema")]
    pub schema: String,
    pub model: Model,
    pub model_load: ModelLoad,
    pub unload_after_idle_min: u32,
    pub input_device: String,
    pub dictation_limit_s: u32,
    pub hands_free_silence_ms: u32,
    pub partials: bool,
    pub insert: Insert,
    pub insert_type_apps: Vec<String>,
    pub restore_clipboard: bool,
    pub sounds: bool,
    pub hud: Hud,
    pub login_item: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Model {
    #[default]
    #[serde(rename = "parakeet-tdt-0.6b-v3-int8")]
    ParakeetV3Int8,
    #[serde(rename = "parakeet-tdt-0.6b-v2-int8")]
    ParakeetV2Int8,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelLoad {
    #[default]
    OnDemand,
    AtLogin,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Insert {
    #[default]
    Paste,
    Type,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Hud {
    #[default]
    Notch,
    Cursor,
    Off,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema: "./schemas/settings.schema.json".to_string(),
            model: Model::default(),
            model_load: ModelLoad::default(),
            unload_after_idle_min: 10,
            input_device: "system".to_string(),
            dictation_limit_s: 120,
            hands_free_silence_ms: 500,
            partials: true,
            insert: Insert::default(),
            insert_type_apps: Vec::new(),
            restore_clipboard: true,
            sounds: true,
            hud: Hud::default(),
            login_item: false,
        }
    }
}

/// A settings error with the 1-based line it points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: usize,
    pub message: String,
}

impl Diagnostic {
    pub fn new(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "settings.json:{}: {}", self.line, self.message)
    }
}

impl std::error::Error for Diagnostic {}

impl Settings {
    /// Parses and validates settings.json text.
    pub fn parse(text: &str) -> Result<Settings, Diagnostic> {
        let value: serde_json::Value = serde_json::from_str(text).map_err(|e| {
            Diagnostic::new(line_of(&e), format!("invalid JSON: {}", strip_position(&e)))
        })?;
        let object = value
            .as_object()
            .ok_or_else(|| Diagnostic::new(1, "must be a JSON object"))?;
        if let Some(key) = object.keys().find(|k| !KNOWN_KEYS.contains(&k.as_str())) {
            return Err(Diagnostic::new(
                key_line(text, key),
                format!("unknown key \"{key}\""),
            ));
        }

        let settings: Settings = serde_json::from_str(text).map_err(|e| {
            let line = line_of(&e);
            let raw = strip_position(&e);
            let message = match key_on_line(text, line) {
                Some(key) => format!("\"{key}\": {raw}"),
                None => raw,
            };
            Diagnostic::new(line, message)
        })?;

        check_range(
            text,
            "dictation_limit_s",
            settings.dictation_limit_s,
            DICTATION_LIMIT_S,
        )?;
        check_range(
            text,
            "hands_free_silence_ms",
            settings.hands_free_silence_ms,
            HANDS_FREE_SILENCE_MS,
        )?;
        for app in &settings.insert_type_apps {
            if !is_bundle_id(app) {
                let line = line_containing(text, &format!("\"{app}\""))
                    .unwrap_or_else(|| key_line(text, "insert_type_apps"));
                return Err(Diagnostic::new(
                    line,
                    format!("\"insert_type_apps\" entry \"{app}\" is not a bundle id"),
                ));
            }
        }
        Ok(settings)
    }
}

fn check_range(
    text: &str,
    key: &str,
    value: u32,
    range: RangeInclusive<u32>,
) -> Result<(), Diagnostic> {
    if range.contains(&value) {
        return Ok(());
    }
    Err(Diagnostic::new(
        key_line(text, key),
        format!(
            "\"{key}\" must be between {} and {}",
            range.start(),
            range.end()
        ),
    ))
}

/// Hand check for `^[A-Za-z][A-Za-z0-9-]*(\.[A-Za-z0-9-]+)+$`.
fn is_bundle_id(s: &str) -> bool {
    let Some((first, rest)) = s.split_once('.') else {
        return false;
    };
    let first_ok =
        first.starts_with(|c: char| c.is_ascii_alphabetic()) && first.chars().all(is_label_char);
    first_ok
        && rest
            .split('.')
            .all(|seg| !seg.is_empty() && seg.chars().all(is_label_char))
}

fn is_label_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-'
}

fn line_of(e: &serde_json::Error) -> usize {
    e.line().max(1)
}

/// The serde_json message without its trailing " at line N column M".
fn strip_position(e: &serde_json::Error) -> String {
    let text = e.to_string();
    let stripped = text
        .rsplit_once(" at line ")
        .map(|(msg, _)| msg.to_string());
    stripped.unwrap_or(text)
}

/// Whether `line` holds the key `"key":` (a key, not a string value).
fn has_key(line: &str, key: &str) -> bool {
    let needle = format!("\"{key}\"");
    line.match_indices(&needle)
        .any(|(pos, _)| line[pos + needle.len()..].trim_start().starts_with(':'))
}

/// 1-based line of the first `"key":` in `text`, or 1 if not found.
fn key_line(text: &str, key: &str) -> usize {
    text.lines()
        .position(|line| has_key(line, key))
        .map_or(1, |i| i + 1)
}

/// The known settings key written on the 1-based `line`, if any.
fn key_on_line(text: &str, line: usize) -> Option<&'static str> {
    let content = text.lines().nth(line.checked_sub(1)?)?;
    KNOWN_KEYS.iter().copied().find(|key| has_key(content, key))
}

/// 1-based line of the first line containing `needle`.
fn line_containing(text: &str, needle: &str) -> Option<usize> {
    text.lines()
        .position(|line| line.contains(needle))
        .map(|i| i + 1)
}

/// A file shipped inside the binary, written to `name` under the config dir on first run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddedFile {
    pub name: &'static str,
    pub contents: &'static str,
}

/// settings.json and its schema, embedded. Other modules add their own files and the caller
/// merges the lists.
pub fn embedded_settings_files() -> [EmbeddedFile; 2] {
    [
        EmbeddedFile {
            name: "settings.json",
            contents: embedded::SETTINGS_DEFAULTS,
        },
        EmbeddedFile {
            name: "schemas/settings.schema.json",
            contents: embedded::SETTINGS_SCHEMA,
        },
    ]
}

/// The config folder: `$JISUNO_CONFIG_DIR`, else `$HOME/.config/jisuno`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDir {
    root: PathBuf,
}

impl ConfigDir {
    pub fn resolve() -> Result<Self> {
        resolve_with(env::var_os("JISUNO_CONFIG_DIR"), env::var_os("HOME"))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn settings_path(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    pub fn keybindings_path(&self) -> PathBuf {
        self.root.join("keybindings.json")
    }

    pub fn theme_path(&self) -> PathBuf {
        self.root.join("theme.json")
    }

    pub fn voice_control_path(&self) -> PathBuf {
        self.root.join("voice-control-config.json")
    }

    pub fn schemas_dir(&self) -> PathBuf {
        self.root.join("schemas")
    }

    /// Creates the config dir and `schemas/`, then writes each file that is missing. Existing
    /// entries are never touched. Returns the paths written.
    pub fn ensure_first_run(&self, files: &[EmbeddedFile]) -> Result<Vec<PathBuf>> {
        let schemas = self.schemas_dir();
        fs::create_dir_all(&schemas).with_context(|| format!("creating {}", schemas.display()))?;
        let mut written = Vec::new();
        for file in files {
            let path = self.root.join(file.name);
            match fs::symlink_metadata(&path) {
                Ok(_) => continue,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(e).with_context(|| format!("checking {}", path.display()));
                }
            }
            write_atomic(&path, file.contents)?;
            written.push(path);
        }
        Ok(written)
    }
}

fn resolve_with(override_dir: Option<OsString>, home: Option<OsString>) -> Result<ConfigDir> {
    if let Some(dir) = override_dir.filter(|d| !d.is_empty()) {
        return Ok(ConfigDir::at(dir));
    }
    let home = home.filter(|h| !h.is_empty()).ok_or_else(|| {
        anyhow!("HOME is not set; set JISUNO_CONFIG_DIR to choose the config dir")
    })?;
    Ok(ConfigDir::at(
        PathBuf::from(home).join(".config").join("jisuno"),
    ))
}

/// Writes `contents` to a unique temp file next to the target, then renames it over the target.
/// A symlink at `path` is followed, so the link itself survives and its target is updated.
/// The existing file's permissions are kept; a new file gets the default mode.
pub fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    let target =
        resolve_write_target(path).with_context(|| format!("resolving {}", path.display()))?;
    if let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let (tmp, file) =
        create_temp(&target).with_context(|| format!("creating temp for {}", path.display()))?;
    if let Err(e) = write_then_rename(file, &tmp, &target, contents) {
        // Best effort: the temp file is only a leftover, the original is untouched.
        let _ = fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("writing {}", path.display()));
    }
    Ok(())
}

/// The file a write to `path` lands in. A symlink resolves to its target so the link is kept.
fn resolve_write_target(path: &Path) -> io::Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {}
        Ok(_) => return Ok(path.to_path_buf()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(path.to_path_buf()),
        Err(e) => return Err(e),
    }
    if let Ok(resolved) = fs::canonicalize(path) {
        return Ok(resolved);
    }
    // Dangling link: follow the chain by hand. A relative target resolves against the
    // link's own dir; `join` keeps an absolute target as is.
    let mut current = path.to_path_buf();
    for _ in 0..MAX_SYMLINK_HOPS {
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {}
            Ok(_) => return Ok(current),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(current),
            Err(e) => return Err(e),
        }
        let next = fs::read_link(&current)?;
        let base = current.parent().unwrap_or_else(|| Path::new(""));
        current = base.join(next);
    }
    Err(io::Error::other("too many levels of symbolic links"))
}

/// Creates `<file name>.<pid>.<counter>.tmp` in `target`'s dir. `create_new` means no writer
/// ever shares a temp file, so a name collision retries with the next counter value.
fn create_temp(target: &Path) -> io::Result<(PathBuf, fs::File)> {
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let dir = target.parent().unwrap_or_else(|| Path::new(""));
    loop {
        let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut tmp_name = name.to_owned();
        tmp_name.push(format!(".{}.{n}.tmp", process::id()));
        let tmp = dir.join(tmp_name);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(file) => return Ok((tmp, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

fn write_then_rename(
    mut file: fs::File,
    tmp: &Path,
    target: &Path,
    contents: &str,
) -> io::Result<()> {
    match fs::metadata(target) {
        Ok(meta) => file.set_permissions(meta.permissions())?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    drop(file);
    fs::rename(tmp, target)?;
    sync_dir(target.parent().unwrap_or_else(|| Path::new("")))
}

/// Makes the rename durable. Directories cannot be fsynced on Windows, so this is unix-only.
#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

/// Keeps the last value that parsed. Hot reload calls `apply` with each new parse result.
#[derive(Debug)]
pub struct LastGood<T> {
    current: T,
    last_error: Option<Diagnostic>,
}

impl<T> LastGood<T> {
    pub fn new(initial: T) -> Self {
        Self {
            current: initial,
            last_error: None,
        }
    }

    pub fn current(&self) -> &T {
        &self.current
    }

    /// On `Ok`, replaces the value. On `Err`, keeps the previous value and returns the
    /// diagnostic to show. A repeat of the error just shown returns `None`, so the HUD shows
    /// it once. A later `Ok` clears it.
    pub fn apply(&mut self, result: Result<T, Diagnostic>) -> Option<Diagnostic> {
        match result {
            Ok(value) => {
                self.current = value;
                self.last_error = None;
                None
            }
            Err(diagnostic) => {
                if self.last_error.as_ref() == Some(&diagnostic) {
                    return None;
                }
                self.last_error = Some(diagnostic.clone());
                Some(diagnostic)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_err(text: &str) -> Diagnostic {
        match Settings::parse(text) {
            Ok(_) => panic!("expected an error for {text}"),
            Err(diagnostic) => diagnostic,
        }
    }

    #[test]
    fn defaults_match_defaults_file() {
        let file: Settings =
            serde_json::from_str(include_str!("../defaults/settings.json")).unwrap();
        assert_eq!(file, Settings::default());
        assert_eq!(
            Settings::parse(include_str!("../defaults/settings.json")).unwrap(),
            Settings::default()
        );
    }

    #[test]
    fn empty_object_is_defaults() {
        assert_eq!(Settings::parse("{}").unwrap(), Settings::default());
    }

    #[test]
    fn parses_overrides_and_enums() {
        let text = r#"{
  "model": "parakeet-tdt-0.6b-v2-int8",
  "model_load": "at_login",
  "insert": "type",
  "hud": "cursor",
  "insert_type_apps": ["com.apple.Terminal", "com.googlecode.iterm2"]
}"#;
        let settings = Settings::parse(text).unwrap();
        assert_eq!(settings.model, Model::ParakeetV2Int8);
        assert_eq!(settings.model_load, ModelLoad::AtLogin);
        assert_eq!(settings.insert, Insert::Type);
        assert_eq!(settings.hud, Hud::Cursor);
        assert_eq!(settings.insert_type_apps.len(), 2);
        assert_eq!(settings.dictation_limit_s, 120);
    }

    #[test]
    fn unknown_key_reports_line() {
        let text = "{\n  \"model\": \"parakeet-tdt-0.6b-v3-int8\",\n  \"bogus\": 1\n}";
        let diagnostic = parse_err(text);
        assert_eq!(diagnostic.line, 3);
        assert_eq!(diagnostic.message, "unknown key \"bogus\"");
        assert_eq!(
            diagnostic.to_string(),
            "settings.json:3: unknown key \"bogus\""
        );
    }

    #[test]
    fn unknown_key_is_not_confused_with_value() {
        let text = "{\n  \"partials\": true,\n  \"nope\": true\n}";
        let diagnostic = parse_err(text);
        assert_eq!(diagnostic.line, 3);
        assert_eq!(diagnostic.message, "unknown key \"nope\"");
    }

    #[test]
    fn dictation_limit_range() {
        assert_eq!(
            Settings::parse("{\"dictation_limit_s\": 10}")
                .unwrap()
                .dictation_limit_s,
            10
        );
        assert_eq!(
            Settings::parse("{\"dictation_limit_s\": 600}")
                .unwrap()
                .dictation_limit_s,
            600
        );
        let low = parse_err("{\n\"dictation_limit_s\": 9\n}");
        assert_eq!(low.line, 2);
        assert_eq!(
            low.to_string(),
            "settings.json:2: \"dictation_limit_s\" must be between 10 and 600"
        );
        let high = parse_err("{\"dictation_limit_s\": 601}");
        assert_eq!(
            high.message,
            "\"dictation_limit_s\" must be between 10 and 600"
        );
    }

    #[test]
    fn hands_free_silence_range() {
        assert!(Settings::parse("{\"hands_free_silence_ms\": 200}").is_ok());
        assert!(Settings::parse("{\"hands_free_silence_ms\": 5000}").is_ok());
        let low = parse_err("{\"hands_free_silence_ms\": 199}");
        assert_eq!(
            low.message,
            "\"hands_free_silence_ms\" must be between 200 and 5000"
        );
        let high = parse_err("{\n\n\"hands_free_silence_ms\": 5001\n}");
        assert_eq!(high.line, 3);
    }

    #[test]
    fn negative_and_fractional_integers_are_rejected() {
        assert!(Settings::parse("{\"unload_after_idle_min\": -1}").is_err());
        assert!(Settings::parse("{\"dictation_limit_s\": 12.5}").is_err());
        assert_eq!(
            Settings::parse("{\"unload_after_idle_min\": 0}")
                .unwrap()
                .unload_after_idle_min,
            0
        );
    }

    #[test]
    fn bad_enum_value_reports_key_and_line() {
        let text = "{\n  \"hud\": \"top\"\n}";
        let diagnostic = parse_err(text);
        assert_eq!(diagnostic.line, 2);
        assert!(diagnostic.message.starts_with("\"hud\": "), "{diagnostic}");
        assert!(
            diagnostic.message.contains("unknown variant"),
            "{diagnostic}"
        );
    }

    #[test]
    fn bundle_id_pattern() {
        for good in ["com.apple.Terminal", "a.b", "com.x-y.Z9", "com.1app.x"] {
            assert!(is_bundle_id(good), "{good}");
        }
        for bad in [
            "",
            "Terminal",
            "com.",
            ".com.apple",
            "com..apple",
            "1com.apple",
            "-com.apple",
            "com.apple_x",
            "com.apple.Term inal",
            "com.apple/x",
        ] {
            assert!(!is_bundle_id(bad), "{bad}");
        }
    }

    #[test]
    fn bad_bundle_id_reports_entry_line() {
        let text =
            "{\n  \"insert_type_apps\": [\n    \"com.apple.Terminal\",\n    \"Terminal\"\n  ]\n}";
        let diagnostic = parse_err(text);
        assert_eq!(diagnostic.line, 4);
        assert_eq!(
            diagnostic.message,
            "\"insert_type_apps\" entry \"Terminal\" is not a bundle id"
        );
    }

    #[test]
    fn syntax_error_reports_line() {
        let diagnostic = parse_err("{\n  \"model\": \n}");
        assert_eq!(diagnostic.line, 3);
        assert!(
            diagnostic.message.starts_with("invalid JSON: "),
            "{diagnostic}"
        );
    }

    #[test]
    fn non_object_is_rejected() {
        let diagnostic = parse_err("[1, 2]");
        assert_eq!(diagnostic.line, 1);
        assert_eq!(diagnostic.message, "must be a JSON object");
    }

    #[test]
    fn resolve_prefers_override_then_home() {
        let over = resolve_with(Some("/tmp/x".into()), Some("/home/u".into())).unwrap();
        assert_eq!(over.root(), Path::new("/tmp/x"));
        let empty_override = resolve_with(Some("".into()), Some("/home/u".into())).unwrap();
        assert_eq!(empty_override.root(), Path::new("/home/u/.config/jisuno"));
        assert!(resolve_with(None, None).is_err());
    }

    #[test]
    fn config_paths() {
        let dir = ConfigDir::at("/c");
        assert_eq!(dir.settings_path(), Path::new("/c/settings.json"));
        assert_eq!(dir.keybindings_path(), Path::new("/c/keybindings.json"));
        assert_eq!(dir.theme_path(), Path::new("/c/theme.json"));
        assert_eq!(
            dir.voice_control_path(),
            Path::new("/c/voice-control-config.json")
        );
        assert_eq!(dir.schemas_dir(), Path::new("/c/schemas"));
    }

    #[test]
    fn first_run_writes_missing_then_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = ConfigDir::at(tmp.path().join("jisuno"));
        let files = embedded_settings_files();

        let written = dir.ensure_first_run(&files).unwrap();
        assert_eq!(written.len(), 2);
        assert_eq!(
            fs::read_to_string(dir.settings_path()).unwrap(),
            include_str!("../defaults/settings.json")
        );
        assert_eq!(
            fs::read_to_string(dir.schemas_dir().join("settings.schema.json")).unwrap(),
            include_str!("../schemas/settings.schema.json")
        );

        let again = dir.ensure_first_run(&files).unwrap();
        assert!(again.is_empty());
    }

    #[test]
    fn first_run_keeps_user_edited_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = ConfigDir::at(tmp.path());
        fs::write(dir.settings_path(), "{\"sounds\": false}\n").unwrap();

        let written = dir.ensure_first_run(&embedded_settings_files()).unwrap();
        assert_eq!(
            written,
            vec![dir.schemas_dir().join("settings.schema.json")]
        );
        assert_eq!(
            fs::read_to_string(dir.settings_path()).unwrap(),
            "{\"sounds\": false}\n"
        );
    }

    #[test]
    fn atomic_write_leaves_no_tmp() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("settings.json");
        write_atomic(&path, "{}\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "{}\n");

        write_atomic(&path, "{\"sounds\": false}\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"sounds\": false}\n");

        let names: Vec<String> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["settings.json".to_string()]);
    }

    fn dir_names(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn concurrent_atomic_writes_never_interleave() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        let a = format!("{{\"tag\": \"{}\"}}\n", "a".repeat(64 * 1024));
        let b = format!("{{\"tag\": \"{}\"}}\n", "b".repeat(64 * 1024));
        std::thread::scope(|s| {
            for body in [&a, &b] {
                let path = &path;
                s.spawn(move || {
                    for _ in 0..100 {
                        write_atomic(path, body).unwrap();
                    }
                });
            }
        });
        let final_text = fs::read_to_string(&path).unwrap();
        assert!(final_text == a || final_text == b, "torn write");
        assert_eq!(dir_names(tmp.path()), vec!["settings.json".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_through_symlink_keeps_link() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("dotfiles").join("settings.json");
        fs::create_dir_all(real.parent().unwrap()).unwrap();
        fs::write(&real, "{}\n").unwrap();
        let link = tmp.path().join("settings.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        write_atomic(&link, "{\"sounds\": false}\n").unwrap();

        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(fs::read_to_string(&real).unwrap(), "{\"sounds\": false}\n");
        assert_eq!(
            dir_names(real.parent().unwrap()),
            vec!["settings.json".to_string()]
        );
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_through_dangling_relative_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let link = tmp.path().join("settings.json");
        std::os::unix::fs::symlink("sub/settings.json", &link).unwrap();
        fs::create_dir(tmp.path().join("sub")).unwrap();

        write_atomic(&link, "{}\n").unwrap();

        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read_to_string(tmp.path().join("sub").join("settings.json")).unwrap(),
            "{}\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_preserves_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        fs::write(&path, "{}\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        write_atomic(&path, "{\"sounds\": false}\n").unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(dir_names(tmp.path()), vec!["settings.json".to_string()]);
    }

    #[test]
    fn last_good_keeps_value_on_error() {
        let mut holder = LastGood::new(Settings::default());
        let good = Settings::parse("{\"sounds\": false}").unwrap();
        assert_eq!(holder.apply(Ok(good.clone())), None);
        assert_eq!(holder.current(), &good);

        let bad = Settings::parse("{\"bogus\": 1}");
        let shown = holder.apply(bad.clone());
        assert_eq!(shown.map(|d| d.line), Some(1));
        assert_eq!(holder.current(), &good);

        // Same error again: not shown twice.
        assert_eq!(holder.apply(bad), None);
        assert_eq!(holder.current(), &good);

        // Fixed, then a new error is shown again.
        assert_eq!(holder.apply(Ok(Settings::default())), None);
        assert_eq!(holder.current(), &Settings::default());
        assert!(holder.apply(Settings::parse("{\"hud\": \"x\"}")).is_some());
    }
}
