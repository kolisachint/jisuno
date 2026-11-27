//! Settings window view models (`app-ui.md` §5): one model per pane plus the
//! key-recording flow from `keybindings.md` §9.
//!
//! Pure: no GPUI dependency, so Linux CI builds and tests this module. The
//! GPUI view (`settings_view`, macOS only) renders [`SettingsModel`] and
//! emits [`SettingsIntent`]s; the app agent hosts it, writes files and feeds
//! updated models back. Key recording works the same way: the view enters
//! recording mode, the host feeds recorded [`Key`]s (from the event tap) to
//! [`feed_recorded_key`], and validation + conflict text come from
//! `jisuno-logic` keybinding APIs.

use jisuno_logic::keybindings::{self, Binding, Command, Key, Keybindings, When};
use jisuno_logic::voice_control::{CommandDef, Config, WindowDef};

/// The eight Settings panes (§5), in sidebar order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pane {
    General,
    Audio,
    Model,
    Keybindings,
    Commands,
    Appearance,
    Performance,
    Files,
}

impl Pane {
    pub fn all() -> [Pane; 8] {
        [
            Pane::General,
            Pane::Audio,
            Pane::Model,
            Pane::Keybindings,
            Pane::Commands,
            Pane::Appearance,
            Pane::Performance,
            Pane::Files,
        ]
    }

    pub fn title(self) -> &'static str {
        match self {
            Pane::General => "General",
            Pane::Audio => "Audio",
            Pane::Model => "Model",
            Pane::Keybindings => "Keybindings",
            Pane::Commands => "Commands",
            Pane::Appearance => "Appearance",
            Pane::Performance => "Performance",
            Pane::Files => "Files",
        }
    }
}

/// General pane: `settings.json` keys (§5 table).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneralModel {
    pub hud: String,
    pub sounds: bool,
    pub partials: bool,
    pub insert: String,
    pub insert_type_apps: Vec<String>,
    pub restore_clipboard: bool,
    pub dictation_limit_s: u32,
    pub hands_free_silence_ms: u32,
    pub login_item: bool,
}

impl Default for GeneralModel {
    fn default() -> Self {
        Self {
            hud: "notch".into(),
            sounds: true,
            partials: true,
            insert: "paste".into(),
            insert_type_apps: Vec::new(),
            restore_clipboard: true,
            dictation_limit_s: 120,
            hands_free_silence_ms: 500,
            login_item: false,
        }
    }
}

/// Audio pane: device picker plus the Bluetooth warning (§5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioModel {
    /// `settings.json` `input_device`; `system` is the system input.
    pub input_device: String,
    /// Connected devices, system input first.
    pub devices: Vec<String>,
    /// Whether the level-meter test is running (host-owned audio).
    pub testing_mic: bool,
}

impl AudioModel {
    /// The Bluetooth warning, shown for Bluetooth inputs (§5).
    pub fn bluetooth_warning(&self) -> Option<&'static str> {
        if self.input_device.to_lowercase().contains("bluetooth") {
            Some("Bluetooth input: audio is compressed and latency is higher. Use the built-in microphone for dictation.")
        } else {
            None
        }
    }

    /// The missing-device note (H17), when the named device is not connected.
    pub fn missing_device_note(&self) -> Option<String> {
        self.input_device
            .strip_suffix(" (not connected)")
            .map(|name| format!("Mic \"{name}\" not found · using system input"))
    }
}

/// The two models on the Model pane (§5): v3 default (~650 MB), v2 optional.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelId {
    V3,
    V2,
}

impl ModelId {
    pub fn file_id(self) -> &'static str {
        match self {
            ModelId::V3 => "parakeet-tdt-0.6b-v3-int8",
            ModelId::V2 => "parakeet-tdt-0.6b-v2-int8",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ModelId::V3 => "Parakeet TDT 0.6B v3, int8",
            ModelId::V2 => "Parakeet TDT 0.6B v2, int8",
        }
    }

    pub fn note(self) -> &'static str {
        match self {
            ModelId::V3 => "default · multilingual · ~650 MB",
            ModelId::V2 => "English only · optional download",
        }
    }
}

/// Model pane: selection, state line, load policy, idle unload, language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPaneModel {
    pub selected: ModelId,
    /// Whether the selected model's file is downloaded.
    pub downloaded: bool,
    /// Whether the model is in memory.
    pub warm: bool,
    /// Whether a load is running.
    pub loading: bool,
    /// A download in progress.
    pub download_percent: Option<u32>,
    pub load_policy: String,
    pub unload_after_idle_min: u32,
    pub language: String,
}

impl ModelPaneModel {
    /// The state line copy (§5 prototype): download progress, presence,
    /// then memory state.
    pub fn state_line(&self) -> String {
        if let Some(percent) = self.download_percent {
            return format!("Downloading {percent}%");
        }
        if !self.downloaded {
            return "Not downloaded".into();
        }
        if self.warm {
            "Downloaded, loaded (warm)".into()
        } else if self.loading {
            "Downloaded, loading".into()
        } else {
            "Downloaded, not loaded".into()
        }
    }
}

/// One Keybindings row: every command in `keybindings.md` §10 is listed; a
/// command without a binding shows `Not set` (§5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindingRow {
    pub command: Command,
    pub key: Option<Key>,
    pub when: Option<When>,
    pub hold: bool,
}

impl KeybindingRow {
    /// The key cell copy: the display form, or `Not set`.
    pub fn key_display(&self) -> String {
        self.key
            .as_ref()
            .map(Key::display)
            .unwrap_or_else(|| "Not set".into())
    }

    /// The When cell copy: the context, or `-` for none recorded.
    pub fn when_display(&self) -> &'static str {
        match self.when {
            Some(When::Listening) => "listening",
            Some(When::Confirming) => "confirming",
            None => "global",
        }
    }

    /// Build the rows from a validated [`Keybindings`]: one row per bindable
    /// command, in `keybindings.md` §10 order.
    pub fn all_from(bindings: &Keybindings) -> Vec<KeybindingRow> {
        const ORDER: [Command; 9] = [
            Command::Dictate,
            Command::VoiceCommand,
            Command::Warm,
            Command::HandsFreeToggle,
            Command::Cancel,
            Command::Confirm,
            Command::Settings,
            Command::Unload,
            Command::Menu,
        ];
        // `voice.cancel` has two bindings (listening + confirming); every
        // other command has at most one.
        let mut rows = Vec::new();
        for command in ORDER {
            let mut found: Vec<&Binding> = bindings
                .bindings
                .iter()
                .filter(|b| b.command == command)
                .collect();
            found.sort_by_key(|b| b.when.map(|w| w as u8));
            if found.is_empty() {
                rows.push(KeybindingRow {
                    command,
                    key: None,
                    when: None,
                    hold: false,
                });
            } else {
                for binding in found {
                    rows.push(KeybindingRow {
                        command,
                        key: Some(binding.key.clone()),
                        when: binding.when,
                        hold: binding.hold,
                    });
                }
            }
        }
        rows
    }
}

/// The in-progress key recording (`keybindings.md` §9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recording {
    pub command: Command,
    pub when: Option<When>,
    /// The live display form (`Press keys…` until the first key).
    pub live_display: String,
}

/// What feeding a recorded key produced (§9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordOutcome {
    /// Still recording (a lone modifier so far): keep showing `live_display`.
    Pending,
    /// Saved. A common-shortcut warning may ride along (still saved, §8).
    Saved { warning: Option<String> },
    /// Clashes with another binding: show `Taken by <command> (<when>)`
    /// with Replace/Cancel.
    Conflict {
        taken_by: Command,
        when: Option<When>,
    },
    /// Rejected: the §7 message, shown inline under the row.
    Error(String),
}

/// Feed one recorded [`Key`] from the host's event tap into a recording.
/// Esc alone (no modifiers) cancels and keeps the old binding (§9).
pub fn feed_recorded_key(model: &mut SettingsModel, key: Key) -> RecordOutcome {
    let recording = match &model.recording {
        Some(recording) => recording.clone(),
        None => return RecordOutcome::Error("no recording in progress".into()),
    };
    // Esc alone cancels the recording; the old binding stays (§9).
    if matches!(
        &key,
        Key::Chord { modifiers, key } if modifiers.is_empty() && key == "escape"
    ) {
        model.recording = None;
        return RecordOutcome::Error("cancelled".into());
    }
    let binding = Binding {
        key: key.clone(),
        hold: matches!(key, Key::Hold(_)),
        when: recording.when,
        command: recording.command,
    };
    let errors = keybindings::validate_binding(&binding);
    if let Some(first) = errors.iter().find(|d| d.is_error()) {
        model.recording = None;
        model.keybinding_error = Some((recording.command, first.message.clone()));
        return RecordOutcome::Error(first.message.clone());
    }
    let current = model.bindings();
    if let Some(other) = current.conflict(&binding.key, binding.when) {
        if other.command != recording.command {
            model.recording = Some(Recording {
                live_display: key.display(),
                ..recording
            });
            model.pending_conflict = Some(PendingConflict {
                key: key.clone(),
                when: recording.when,
                taken_by: other.command,
            });
            return RecordOutcome::Conflict {
                taken_by: other.command,
                when: other.when,
            };
        }
    }
    let warning = common_shortcut_warning(&binding);
    model.apply_binding(binding);
    model.recording = None;
    model.pending_conflict = None;
    model.keybinding_error = None;
    if warning.is_some() {
        model.keybinding_warning = Some((recording.command, warning.clone().unwrap()));
    }
    RecordOutcome::Saved { warning }
}

/// A shown conflict: `Taken by <command> (<when>)` with Replace/Cancel (§9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingConflict {
    pub key: Key,
    pub when: Option<When>,
    pub taken_by: Command,
}

impl PendingConflict {
    /// The inline conflict copy (§9).
    pub fn taken_by_line(&self) -> String {
        let when = self.taken_by_when().unwrap_or("global");
        format!("Taken by {} ({})", self.taken_by.id(), when)
    }

    fn taken_by_when(&self) -> Option<&'static str> {
        match self.when {
            Some(When::Listening) => Some("listening"),
            Some(When::Confirming) => Some("confirming"),
            None => None,
        }
    }
}

/// The common-shortcut warning (§8) via the public parse API: a synthesized
/// single-binding file keeps the known-list check in `jisuno-logic`.
fn common_shortcut_warning(binding: &Binding) -> Option<String> {
    let when = match binding.when {
        Some(When::Listening) => ", \"when\": \"listening\"",
        Some(When::Confirming) => ", \"when\": \"confirming\"",
        None => "",
    };
    let hold = if binding.hold { ", \"hold\": true" } else { "" };
    let text = format!(
        "{{\"bindings\": [{{\"key\": \"{}\", \"command\": \"{}\"{}{}}}]}}",
        binding.key.file(),
        binding.command.id(),
        hold,
        when
    );
    match keybindings::parse_with_warnings(&text) {
        Ok((_, warnings)) => warnings.into_iter().map(|d| d.message).next(),
        Err(_) => None,
    }
}

/// One Commands row: a readable view over a [`CommandDef`] (read-only, §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRow {
    pub say: Vec<String>,
    pub steps: String,
    pub confirm: bool,
    pub agent: bool,
}

impl CommandRow {
    pub fn of(command: &CommandDef) -> Self {
        let steps = command
            .steps
            .iter()
            .map(|step| {
                serde_json::to_value(step)
                    .ok()
                    .and_then(|v| {
                        let do_ = v.get("do")?.as_str()?.to_string();
                        let mut args: Vec<String> = v
                            .as_object()?
                            .iter()
                            .filter(|(k, _)| *k != "do")
                            .map(|(_, v)| {
                                v.as_str()
                                    .map(str::to_string)
                                    .unwrap_or_else(|| v.to_string())
                            })
                            .collect();
                        args.sort();
                        args.insert(0, do_);
                        Some(args.join(" "))
                    })
                    .unwrap_or_else(|| "?".into())
            })
            .collect::<Vec<_>>()
            .join(" → ");
        Self {
            say: command.say.clone(),
            steps,
            confirm: command.confirm,
            agent: command.agent,
        }
    }
}

/// One workspace window row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRow {
    pub workspace: String,
    pub app: String,
    pub opens: String,
    pub place: String,
}

impl WorkspaceRow {
    pub fn of(workspace: &str, window: &WindowDef) -> Self {
        Self {
            workspace: workspace.to_string(),
            app: window.app.clone(),
            opens: window
                .run
                .clone()
                .or_else(|| window.open.clone())
                .unwrap_or_default(),
            place: window.place.as_str().to_string(),
        }
    }
}

/// Split a validated [`Config`] into built-in and agent-added rows
/// (`voice-control.md` §8: agent-added commands show separately).
pub fn command_rows(config: &Config) -> (Vec<CommandRow>, Vec<CommandRow>) {
    let mut builtin = Vec::new();
    let mut agent = Vec::new();
    for command in &config.commands {
        let row = CommandRow::of(command);
        if command.agent {
            agent.push(row);
        } else {
            builtin.push(row);
        }
    }
    (builtin, agent)
}

/// Appearance pane: `theme.json` keys (§5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppearanceModel {
    pub name: String,
    pub appearance: String,
    pub accent_dictate: String,
    pub accent_command: String,
    pub hud_style: String,
    pub show_latency: bool,
}

impl Default for AppearanceModel {
    fn default() -> Self {
        Self {
            name: "quiet-light".into(),
            appearance: "system".into(),
            accent_dictate: "#3a85f7".into(),
            accent_command: "#e39b17".into(),
            hud_style: "island".into(),
            show_latency: false,
        }
    }
}

impl AppearanceModel {
    /// Validate an accent field: `#rrggbb`, lowercase (§5).
    pub fn accent_error(field: &str, value: &str) -> Option<String> {
        let hex = value.strip_prefix('#').unwrap_or("");
        if value.starts_with('#')
            && hex.len() == 6
            && hex.chars().all(|c| c.is_ascii_hexdigit())
            && value == value.to_lowercase()
        {
            None
        } else {
            Some(format!("{field} must be #rrggbb (lowercase)"))
        }
    }
}

/// One Performance row: the metric, its target and the measured value.
/// Values come from `jisuno bench`; unmeasured rows show `not measured`
/// (never simulated numbers, §5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerfRow {
    pub metric: &'static str,
    pub target: &'static str,
    pub measured: Option<String>,
}

impl PerfRow {
    pub fn measured_display(&self) -> &str {
        self.measured.as_deref().unwrap_or("not measured")
    }
}

/// The Performance table (`jisuno.md` §6.1, `app-ui.md` §5.2).
pub fn performance_rows(measured: &[Option<String>; 12]) -> Vec<PerfRow> {
    const ROWS: [(&str, &str); 12] = [
        ("Key-up → text, warm, 5 s clip", "≤ 150 ms"),
        ("Tail flush", "≤ 15 ms"),
        ("Final decode, 5 s clip", "≤ 100 ms"),
        ("Post-process", "≤ 2 ms"),
        ("Insert (paste + clipboard restore)", "≤ 10 ms"),
        ("Key-up → text, cold", "no target set"),
        ("Warm decode, 2 s / 15 s clips", "no target set"),
        ("Cold load", "≤ 1.5 s"),
        ("Memory, warm", "< 1.2 GB"),
        ("Memory, unloaded", "< 60 MB"),
        ("Idle CPU", "no target set"),
        ("WER, LibriSpeech test-clean / own set", "no target set"),
    ];
    ROWS.into_iter()
        .zip(measured.iter())
        .map(|((metric, target), measured)| PerfRow {
            metric,
            target,
            measured: measured.clone(),
        })
        .collect()
}

/// PASS/FAIL for a measured row against its target: `None` while unmeasured.
pub fn perf_pass(row: &PerfRow) -> Option<bool> {
    None.or_else(|| perf_pass_value(row))
}

fn perf_pass_value(_row: &PerfRow) -> Option<bool> {
    // No parsing of bench output yet: the host reports PASS/FAIL per row via
    // `PerfRow` extensions when `jisuno bench` gains machine-readable output.
    None
}

/// One Files row: name, path under `~/.config/jisuno/`, status and the last
/// error line for `Copy error` (§5, §8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    pub name: &'static str,
    pub path: String,
    /// The §8 error copy (`<file>:<line>: <message> · using last good
    /// config`); `None` means `OK`.
    pub error: Option<String>,
}

impl FileRow {
    pub fn status(&self) -> &'static str {
        if self.error.is_some() {
            "Error"
        } else {
            "OK"
        }
    }
}

/// The four config files plus the schemas folder (§5).
pub fn file_rows(config_dir: &str, errors: &[(&'static str, String)]) -> Vec<FileRow> {
    const NAMES: [&str; 4] = [
        "settings.json",
        "keybindings.json",
        "theme.json",
        "voice-control-config.json",
    ];
    NAMES
        .into_iter()
        .map(|name| FileRow {
            name,
            path: format!("{config_dir}/{name}"),
            error: errors
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, message)| message.clone()),
        })
        .collect()
}

/// The Settings window model: the selected pane plus one sub-model per pane.
/// The host fills it from the config files and writes back through intents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsModel {
    pub pane: Pane,
    pub general: GeneralModel,
    pub audio: AudioModel,
    pub model: ModelPaneModel,
    /// Current binding rows (one per command, `voice.cancel` twice).
    pub keybinding_rows: Vec<KeybindingRow>,
    pub recording: Option<Recording>,
    pub pending_conflict: Option<PendingConflict>,
    /// Inline §7 error under the failing row: (command id, message).
    pub keybinding_error: Option<(Command, String)>,
    /// Saved-with-warning notice: (command id, warning).
    pub keybinding_warning: Option<(Command, String)>,
    pub builtin_commands: Vec<CommandRow>,
    pub agent_commands: Vec<CommandRow>,
    pub workspaces: Vec<WorkspaceRow>,
    pub appearance: AppearanceModel,
    pub perf: Vec<PerfRow>,
    /// `jisuno bench` is running (host-owned).
    pub bench_running: bool,
    pub files: Vec<FileRow>,
}

impl SettingsModel {
    /// Rebuild the [`Keybindings`] the rows describe, for conflict checks.
    pub fn bindings(&self) -> Keybindings {
        Keybindings {
            bindings: self
                .keybinding_rows
                .iter()
                .filter_map(|row| {
                    row.key.clone().map(|key| Binding {
                        key,
                        hold: row.hold,
                        when: row.when,
                        command: row.command,
                    })
                })
                .collect(),
        }
    }

    /// Apply a validated binding: set the row, clearing any other row that
    /// held the same key+when (Replace semantics, §9).
    fn apply_binding(&mut self, binding: Binding) {
        self.keybinding_rows.retain(|row| {
            !(row.command != binding.command
                && row.key.as_ref() == Some(&binding.key)
                && row.when == binding.when)
        });
        if let Some(row) = self
            .keybinding_rows
            .iter_mut()
            .find(|row| row.command == binding.command && row.when == binding.when)
        {
            row.key = Some(binding.key);
            row.hold = binding.hold;
        } else {
            self.keybinding_rows.push(KeybindingRow {
                command: binding.command,
                key: Some(binding.key),
                when: binding.when,
                hold: binding.hold,
            });
        }
    }

    /// A gallery/sample model: defaults everywhere, one conflict-free
    /// recording target, built-in + agent commands, unmeasured perf.
    pub fn sample() -> Self {
        let bindings = keybindings::defaults();
        let config = Config {
            schema: None,
            version: 1,
            commands: vec![
                CommandDef {
                    id: "menu".into(),
                    say: vec!["open jisuno".into()],
                    steps: vec![jisuno_logic::voice_control::Step::JisunoMenu],
                    confirm: false,
                    when: None,
                    slots: Default::default(),
                    agent: false,
                },
                CommandDef {
                    id: "note".into(),
                    say: vec!["open note".into()],
                    steps: vec![jisuno_logic::voice_control::Step::JisunoSettings],
                    confirm: false,
                    when: None,
                    slots: Default::default(),
                    agent: true,
                },
            ],
            workspaces: Default::default(),
        };
        let (builtin_commands, agent_commands) = command_rows(&config);
        Self {
            pane: Pane::General,
            general: GeneralModel::default(),
            audio: AudioModel {
                input_device: "system".into(),
                devices: vec!["system".into(), "MacBook Air Microphone".into()],
                testing_mic: false,
            },
            model: ModelPaneModel {
                selected: ModelId::V3,
                downloaded: true,
                warm: false,
                loading: false,
                download_percent: None,
                load_policy: "on_demand".into(),
                unload_after_idle_min: 10,
                language: "auto".into(),
            },
            keybinding_rows: KeybindingRow::all_from(&bindings),
            recording: None,
            pending_conflict: None,
            keybinding_error: None,
            keybinding_warning: None,
            builtin_commands,
            agent_commands,
            workspaces: Vec::new(),
            appearance: AppearanceModel::default(),
            perf: performance_rows(&Default::default()),
            bench_running: false,
            files: file_rows("~/.config/jisuno", &[]),
        }
    }
}

/// Typed intents the host must implement. The view never touches files or
/// the OS itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsIntent {
    /// Write one control: `key` is the file key (`hud`, `input_device`,
    /// `model`, `accent_dictate`, …), `file` names the file.
    SetSetting {
        file: &'static str,
        key: String,
        value: serde_json::Value,
    },
    /// The key cell was clicked: enter recording mode for this command.
    StartRecording {
        command: Command,
        when: Option<When>,
    },
    /// Replace the conflicting binding (the other command goes `Not set`).
    ReplaceBinding,
    CancelRecording,
    /// `Set to default` for one command (conflicts handled as in Replace).
    ResetBinding(Command),
    OpenFile(&'static str),
    /// Copy the Files row error.
    CopyError(&'static str),
    DownloadModel(&'static str),
    RemoveModel(&'static str),
    WarmModel,
    UnloadModel,
    /// Run the mic level meter (nothing recorded).
    TestMic,
    /// Run `jisuno bench` on the reference Mac.
    RunBench,
    SelectPane(Pane),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panes_come_in_sidebar_order() {
        let titles: Vec<&str> = Pane::all().iter().map(|p| p.title()).collect();
        assert_eq!(
            titles,
            [
                "General",
                "Audio",
                "Model",
                "Keybindings",
                "Commands",
                "Appearance",
                "Performance",
                "Files"
            ]
        );
    }

    #[test]
    fn every_bindable_command_has_a_row() {
        let model = SettingsModel::sample();
        // 9 commands, voice.cancel twice (listening + confirming).
        assert_eq!(model.keybinding_rows.len(), 10);
        let unbound: Vec<_> = model
            .keybinding_rows
            .iter()
            .filter(|row| row.key.is_none())
            .map(|row| row.command.id())
            .collect();
        assert_eq!(unbound, ["model.unload", "jisuno.menu"]);
        let cancel: Vec<_> = model
            .keybinding_rows
            .iter()
            .filter(|row| row.command == Command::Cancel)
            .map(|row| row.when_display())
            .collect();
        assert_eq!(cancel, ["listening", "confirming"]);
    }

    #[test]
    fn recording_saves_a_simple_rebind() {
        let mut model = SettingsModel::sample();
        model.recording = Some(Recording {
            command: Command::Warm,
            when: None,
            live_display: "Press keys…".into(),
        });
        let key = Key::parse("ctrl+option+w").unwrap();
        // Re-saving the same key is not a conflict.
        let outcome = feed_recorded_key(&mut model, key);
        assert!(matches!(outcome, RecordOutcome::Saved { .. }));
        assert_eq!(model.recording, None);
    }

    #[test]
    fn recording_reports_conflicts_with_taken_by() {
        let mut model = SettingsModel::sample();
        // `ctrl+option+,` belongs to jisuno.settings: rebinding Warm to it
        // must report the conflict, not overwrite.
        model.recording = Some(Recording {
            command: Command::Warm,
            when: None,
            live_display: "Press keys…".into(),
        });
        let outcome = feed_recorded_key(&mut model, Key::parse("ctrl+option+,").unwrap());
        match outcome {
            RecordOutcome::Conflict { taken_by, when } => {
                assert_eq!(taken_by, Command::Settings);
                assert_eq!(when, None);
            }
            other => panic!("expected a conflict, got {other:?}"),
        }
        let pending = model.pending_conflict.as_ref().unwrap();
        assert_eq!(pending.taken_by_line(), "Taken by jisuno.settings (global)");
        // The old Warm binding stays until Replace.
        let warm = model
            .keybinding_rows
            .iter()
            .find(|row| row.command == Command::Warm)
            .unwrap();
        assert_eq!(warm.key_display(), "⌃⌥W");
    }

    #[test]
    fn recording_rejects_reserved_keys_inline() {
        let mut model = SettingsModel::sample();
        model.recording = Some(Recording {
            command: Command::Warm,
            when: None,
            live_display: "Press keys…".into(),
        });
        let outcome = feed_recorded_key(&mut model, Key::parse("cmd+space").unwrap());
        match outcome {
            RecordOutcome::Error(message) => {
                assert!(message.contains("Spotlight"), "{message}");
            }
            other => panic!("expected an error, got {other:?}"),
        }
        assert_eq!(model.recording, None);
        assert!(model.keybinding_error.is_some());
    }

    #[test]
    fn esc_cancels_and_keeps_the_old_binding() {
        let mut model = SettingsModel::sample();
        model.recording = Some(Recording {
            command: Command::Warm,
            when: None,
            live_display: "Press keys…".into(),
        });
        let outcome = feed_recorded_key(&mut model, Key::parse("escape").unwrap());
        assert!(matches!(outcome, RecordOutcome::Error(_)));
        let warm = model
            .keybinding_rows
            .iter()
            .find(|row| row.command == Command::Warm)
            .unwrap();
        assert_eq!(warm.key_display(), "⌃⌥W");
    }

    #[test]
    fn common_shortcuts_warn_but_save() {
        let warning = common_shortcut_warning(&Binding {
            key: Key::parse("cmd+w").unwrap(),
            hold: false,
            when: None,
            command: Command::Warm,
        });
        let warning = warning.expect("cmd+w must warn");
        assert!(warning.contains("Close"), "{warning}");
        assert!(warning.starts_with("Warning:"), "{warning}");
    }

    #[test]
    fn bluetooth_and_missing_device_notes() {
        let bt = AudioModel {
            input_device: "Bluetooth Headset".into(),
            devices: vec![],
            testing_mic: false,
        };
        assert!(bt.bluetooth_warning().unwrap().contains("compressed"));
        let wired = AudioModel {
            input_device: "system".into(),
            devices: vec![],
            testing_mic: false,
        };
        assert_eq!(wired.bluetooth_warning(), None);
        let missing = AudioModel {
            input_device: "Scarlett 2i2 (not connected)".into(),
            devices: vec![],
            testing_mic: false,
        };
        assert_eq!(
            missing.missing_device_note().as_deref(),
            Some("Mic \"Scarlett 2i2\" not found · using system input")
        );
        assert_eq!(wired.missing_device_note(), None);
    }

    #[test]
    fn model_state_lines() {
        let mut pane = SettingsModel::sample().model;
        assert_eq!(pane.state_line(), "Downloaded, not loaded");
        pane.warm = true;
        assert_eq!(pane.state_line(), "Downloaded, loaded (warm)");
        pane.downloaded = false;
        pane.warm = false;
        assert_eq!(pane.state_line(), "Not downloaded");
        pane.downloaded = false;
        pane.download_percent = Some(42);
        assert_eq!(pane.state_line(), "Downloading 42%");
        assert_eq!(ModelId::V3.file_id(), "parakeet-tdt-0.6b-v3-int8");
        assert_eq!(ModelId::V2.label(), "Parakeet TDT 0.6B v2, int8");
    }

    #[test]
    fn commands_split_builtin_and_agent() {
        let model = SettingsModel::sample();
        assert_eq!(model.builtin_commands.len(), 1);
        assert_eq!(model.agent_commands.len(), 1);
        assert!(!model.builtin_commands[0].agent);
        assert!(model.agent_commands[0].agent);
        assert!(model.builtin_commands[0].steps.contains("jisuno.menu"));
    }

    #[test]
    fn performance_table_has_twelve_ungated_rows() {
        let perf = performance_rows(&Default::default());
        assert_eq!(perf.len(), 12);
        assert_eq!(perf[0].target, "≤ 150 ms");
        assert_eq!(perf[7].target, "≤ 1.5 s");
        assert_eq!(perf[8].target, "< 1.2 GB");
        assert_eq!(perf[9].target, "< 60 MB");
        assert!(perf
            .iter()
            .all(|row| row.measured_display() == "not measured"));
        assert_eq!(perf_pass(&perf[0]), None);
    }

    #[test]
    fn files_rows_flag_errors() {
        let rows = file_rows(
            "~/.config/jisuno",
            &[(
                "keybindings.json",
                "keybindings.json:12: unknown key \"hald\" · using last good config".into(),
            )],
        );
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].status(), "OK");
        assert_eq!(rows[1].status(), "Error");
        assert!(rows[1].path.ends_with("/keybindings.json"));
        assert!(rows[1].error.as_ref().unwrap().contains("hald"));
    }

    #[test]
    fn accent_validation_matches_theme_rules() {
        assert_eq!(
            AppearanceModel::accent_error("accent_dictate", "#3a85f7"),
            None
        );
        assert!(AppearanceModel::accent_error("accent_dictate", "red").is_some());
        assert!(AppearanceModel::accent_error("accent_dictate", "#3A85F7").is_some());
    }
}
