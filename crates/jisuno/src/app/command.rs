//! Command mode: voice-control config, `Runtime`, `AppHooks`, async execution.
//!
//! The tick thread never runs steps: [`AppState::run_command`] ships the
//! resolved [`Plan`][runner::Plan] plus a workspace snapshot to a spawned
//! thread, which drives [`MacActions`][actions::MacActions] and reports the
//! [`Outcome`][runner::Outcome] back over [`CommandDone`]. Hook requests that
//! need the main thread (model warm/unload, menu open) travel back over
//! [`HookRequest`] and are drained on the tick.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use gpui::App;
use jisuno_logic::hold::Input;
use jisuno_logic::runner::{self, Commands, Outcome, Plan, Resolution};
use jisuno_logic::settings::ConfigDir;
use jisuno_logic::voice_control::{self, Config, WindowDef};
use jisuno_platform::actions::{self, MacActions};
use jisuno_ui::hud_model::HudState as UiHud;

use super::state::{text_summary, AppState};
use super::windows::COMMAND_RECENT_PREFIX;
use crate::runtime::{build_runtime, scan_installed_apps, THEMES};

/// Installed-apps cache TTL: re-scanned at most once an hour.
const INSTALLED_TTL: Duration = Duration::from_secs(3600);

/// Work the command thread cannot do itself: model warm/unload needs the
/// tick's generation bookkeeping, and the status menu must open on the main
/// thread.
pub(crate) enum HookRequest {
    WarmModel,
    UnloadModel,
    OpenMenu,
}

/// A finished command run, back on the tick thread for HUD + Recent.
pub(crate) struct CommandDone {
    pub(crate) label: String,
    pub(crate) outcome: Outcome,
}

/// The app-owned half of [`actions::AppHooks`]: theme/model/menu/settings
/// steps delegate here. Runs on the command worker thread; anything needing
/// the tick thread goes through [`HookRequest`].
pub(crate) struct CommandHooks {
    start: Instant,
    theme_path: PathBuf,
    model_name: String,
    hook_tx: mpsc::Sender<HookRequest>,
}

impl CommandHooks {
    pub(crate) fn new(
        start: Instant,
        theme_path: PathBuf,
        model_name: String,
        hook_tx: mpsc::Sender<HookRequest>,
    ) -> Self {
        Self {
            start,
            theme_path,
            model_name,
            hook_tx,
        }
    }

    fn log(&self, msg: &str) {
        let e = self.start.elapsed();
        eprintln!(
            "[{:02}:{:02}.{:03}] {msg}",
            e.as_secs() / 60,
            e.as_secs() % 60,
            e.subsec_millis()
        );
    }
}

impl actions::AppHooks for CommandHooks {
    fn set_theme(&mut self, name: &str) -> Result<(), String> {
        if !THEMES.contains(&name) {
            return Err(format!("unknown theme \"{name}\""));
        }
        let text = std::fs::read_to_string(&self.theme_path).map_err(|e| format!("{e:#}"))?;
        let mut value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("invalid JSON: {e}"))?;
        value
            .as_object_mut()
            .ok_or_else(|| "theme.json must be an object".to_string())?
            .insert("name".to_string(), serde_json::Value::from(name));
        let out = serde_json::to_string_pretty(&value).map_err(|e| format!("{e:#}"))?;
        jisuno_logic::settings::write_atomic(&self.theme_path, &out)
            .map_err(|e| format!("{e:#}"))?;
        self.log(&format!("theme set to {name}"));
        Ok(())
    }

    fn warm_model(&mut self) -> Result<(), String> {
        let ready = jisuno_asr::setup::Model::parse(&self.model_name)
            .map(|m| m.is_ready())
            .unwrap_or(false);
        if !ready {
            return Err("model not installed".to_string());
        }
        let _ = self.hook_tx.send(HookRequest::WarmModel);
        Ok(())
    }

    fn unload_model(&mut self) -> Result<(), String> {
        let _ = self.hook_tx.send(HookRequest::UnloadModel);
        Ok(())
    }

    fn open_menu(&mut self) -> Result<(), String> {
        let _ = self.hook_tx.send(HookRequest::OpenMenu);
        Ok(())
    }

    fn open_settings(&mut self) -> Result<(), String> {
        self.log("settings window not hosted yet");
        Ok(())
    }
}

/// Load the voice-control config: the file, or the embedded defaults plus
/// the first error to show when the file is invalid.
pub(crate) fn load_voice_config(dir: &ConfigDir) -> (Config, Option<String>) {
    let path = dir.voice_control_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    if text.is_empty() {
        return (voice_defaults(), None);
    }
    match voice_control::parse(&text) {
        Ok((config, _)) => (config, None),
        Err(errors) => {
            let first = errors
                .first()
                .map(|d| {
                    format!(
                        "{} · using last good config",
                        d.render("voice-control-config.json")
                    )
                })
                .unwrap_or_else(|| {
                    "voice-control-config.json:1: invalid file · using last good config".to_string()
                });
            (voice_defaults(), Some(first))
        }
    }
}

fn voice_defaults() -> Config {
    voice_control::defaults().unwrap_or(Config {
        schema: None,
        version: 1,
        commands: Vec::new(),
        workspaces: BTreeMap::new(),
    })
}

/// Split `Could not run "<label>": <reason>` back into its parts; fall
/// back to the whole line as the reason when the shape is unexpected.
fn split_resolve_failed(hud: &str) -> (String, String) {
    hud.strip_prefix("Could not run \"")
        .and_then(|rest| rest.split_once("\": "))
        .map(|(label, reason)| (label.to_string(), reason.to_string()))
        .unwrap_or_else(|| (String::new(), hud.to_string()))
}

/// Log-safe command name: the id and the label summary. The label carries
/// spoken slot text, so it is only shown with `JISUNO_LOG_TEXT=1`.
fn command_log(plan: &Plan) -> String {
    format!("{} ({})", plan.command_id, text_summary(&plan.label))
}

/// `Did you mean "x"?` carries the full copy; the HUD needs the phrase.
pub(crate) fn extract_suggestion(copy: &str) -> Option<String> {
    copy.strip_prefix("Did you mean \"")?
        .strip_suffix("\"?")
        .map(str::to_string)
}

impl AppState {
    /// The matcher bridge for this utterance: live voice-control config plus
    /// a fresh [`Runtime`][runner::Runtime] (running app names from
    /// `NSWorkspace`, installed apps cached hourly, installed themes).
    pub(crate) fn voice_commands(&mut self) -> Commands {
        if self.installed_at.elapsed() >= INSTALLED_TTL {
            self.installed_apps = scan_installed_apps(&crate::runtime::applications_dirs());
            self.installed_at = Instant::now();
        }
        let running = jisuno_platform::frontmost::running_app_names();
        let runtime = build_runtime(running, self.installed_apps.clone());
        Commands::new(&self.voice_config, &runtime)
    }

    fn workspaces(&self) -> BTreeMap<String, Vec<WindowDef>> {
        self.voice_config.workspaces.clone()
    }

    /// Queue a resolved plan on a worker thread. Never blocks the tick: the
    /// [`CommandDone`] lands in [`AppState::drain_cmd_done`].
    pub(crate) fn run_command(&mut self, cx: &mut App) {
        let Some(plan) = self.pending_plan.take() else {
            self.log("run command with no plan — ignored");
            return;
        };
        self.confirming = None;
        let label = plan.label.clone();
        self.hide_hud(cx);
        let workspaces = self.workspaces();
        let theme_path = ConfigDir::resolve()
            .map(|d| d.theme_path())
            .unwrap_or_else(|_| PathBuf::from("theme.json"));
        let hooks = CommandHooks::new(
            self.start,
            theme_path,
            self.model_name.clone(),
            self.hook_tx.clone(),
        );
        let tx = self.cmd_tx.clone();
        self.log(&format!("command run: {}", command_log(&plan)));
        std::thread::spawn(move || {
            let mut actions = MacActions::new(hooks);
            let outcome = runner::execute(&plan, &mut actions, &workspaces);
            let _ = tx.send(CommandDone { label, outcome });
        });
    }

    /// Replace decoding with the H10 confirm prompt and arm the timeout.
    /// The confirm HUD shows the runner's exact prompt copy
    /// (`Plan::confirm_prompt`, voice-control.md §6): `detail_lines`
    /// renders `Run "<command>"? <first_step> · …`, so `first_step` is the
    /// prompt's summary part verbatim, not just `steps[0]`.
    pub(crate) fn confirm_summary(plan: &Plan) -> String {
        let prompt = plan.confirm_prompt();
        let prefix = format!("Run \"{}\"? ", plan.label);
        let suffix = " · Return to run · Esc to cancel";
        prompt
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix(suffix))
            .map(str::to_string)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                plan.steps
                    .iter()
                    .map(|step| step.summary())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
    }

    pub(crate) fn show_confirm(&mut self, cx: &mut App, label: String) {
        self.confirming = self.pending_plan.clone();
        let first_step = self
            .confirming
            .as_ref()
            .map(Self::confirm_summary)
            .unwrap_or_default();
        let logged = self
            .confirming
            .as_ref()
            .map(command_log)
            .unwrap_or_default();
        self.log(&format!("confirm prompt: {logged}"));
        if !self.hud_enabled() {
            return;
        }
        let Some(origin) = self.hud_origin(cx) else {
            return;
        };
        let state = UiHud::Confirm {
            command: label,
            first_step,
        };
        self.replace_hud(cx, state, origin, true);
    }

    /// Main-thread hook requests from command worker threads.
    pub(crate) fn drain_hook_requests(&mut self, cx: &mut App) {
        while let Ok(req) = self.hook_rx.try_recv() {
            match req {
                HookRequest::WarmModel => self.start_load(cx, "command"),
                HookRequest::UnloadModel => self.unload_model("command"),
                HookRequest::OpenMenu => self.status.pop_menu(),
            }
        }
    }

    /// Finished command runs: H8 on success, H12 plus a `failed` Recent row
    /// on the first failing step.
    pub(crate) fn drain_cmd_done(&mut self, cx: &mut App) {
        while let Ok(done) = self.cmd_rx.try_recv() {
            match done.outcome {
                Outcome::Done => {
                    self.push_recent(format!("{COMMAND_RECENT_PREFIX}{}", done.label), None);
                    self.show_ephemeral(cx, UiHud::RanCommand { label: done.label }, "command ran");
                }
                Outcome::Failed {
                    step_index,
                    total,
                    action,
                    reason,
                    ..
                } => {
                    self.log(&format!(
                        "step {step_index} of {total} failed: {action} ({})",
                        text_summary(&reason)
                    ));
                    self.push_recent(
                        format!("{COMMAND_RECENT_PREFIX}{}", done.label),
                        Some("failed".to_string()),
                    );
                    self.show_ephemeral(
                        cx,
                        UiHud::StepFailed {
                            step: step_index as u32,
                            total: total as u32,
                            action,
                            reason,
                        },
                        "step failed",
                    );
                }
            }
        }
    }

    /// Resolve one command utterance: feed the machine and show the HUD.
    pub(crate) fn resolve_command(
        &mut self,
        cx: &mut App,
        id: u64,
        text: String,
        frontmost: Option<String>,
    ) {
        use Resolution as R;
        // The machine pairs replies by id: if the transcribe timeout already
        // fired, it ignores the reply — drop the plan/result instead of
        // running or showing anything stale. Recent keeps the utterance
        // either way; it is host history, not machine state.
        match self.voice_commands().resolve(&text, frontmost.as_deref()) {
            R::Run(plan) => {
                self.pending_plan = Some(plan.clone());
                if !self.input_checked(
                    cx,
                    Input::CommandResolved {
                        needs_confirm: false,
                        label: plan.label.clone(),
                        id,
                    },
                    "command resolved",
                ) {
                    self.log(&format!("stale command {id} (machine moved on) — dropped"));
                    self.pending_plan = None;
                }
            }
            R::Confirm(plan) => {
                self.pending_plan = Some(plan.clone());
                if !self.input_checked(
                    cx,
                    Input::CommandResolved {
                        needs_confirm: true,
                        label: plan.label.clone(),
                        id,
                    },
                    "command resolved (confirm)",
                ) {
                    self.log(&format!("stale command {id} (machine moved on) — dropped"));
                    self.pending_plan = None;
                }
            }
            R::DidYouMean(copy) => {
                if self.input_checked(cx, Input::Finished { hud: None, id }, "finished (no match)")
                {
                    self.push_recent(text.clone(), Some("failed".to_string()));
                    let suggestion = extract_suggestion(&copy).unwrap_or(copy);
                    self.show_ephemeral(cx, UiHud::NoMatch { suggestion }, "no match");
                } else {
                    self.push_recent(text.clone(), Some("failed".to_string()));
                }
            }
            R::NoMatch { hud } => {
                self.log(&format!("{hud} for {}", text_summary(&text)));
                self.push_recent(text.clone(), Some("failed".to_string()));
                if self.input_checked(cx, Input::Finished { hud: None, id }, "finished (no match)")
                {
                    self.show_ephemeral(cx, UiHud::NoCommandMatched, "no command matched");
                }
            }
            R::Failed { hud } => {
                // Unreachable with a validated config: log + Recent either
                // way, HUD only while the machine is still waiting.
                self.log(&format!("could not run command ({})", text_summary(&hud)));
                self.push_recent(text.clone(), Some("failed".to_string()));
                let (label, reason) = split_resolve_failed(&hud);
                if self.input_checked(
                    cx,
                    Input::Finished { hud: None, id },
                    "finished (resolve failed)",
                ) {
                    self.show_ephemeral(
                        cx,
                        UiHud::ResolveFailed { label, reason },
                        "resolve failed",
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AppState;
    use jisuno_logic::runner::{Plan, ResolvedStep};

    fn plan(label: &str, steps: Vec<ResolvedStep>) -> Plan {
        Plan {
            command_id: "test".to_string(),
            label: label.to_string(),
            steps,
        }
    }

    /// The confirm HUD carries the runner's exact prompt summary
    /// (voice-control.md §6), not just `steps[0]`.
    #[test]
    fn confirm_shows_runner_prompt_summary() {
        let shell = plan(
            "clean build",
            vec![ResolvedStep::ShellRun {
                cmd: "cargo clean".to_string(),
                cwd: None,
            }],
        );
        assert_eq!(AppState::confirm_summary(&shell), "cargo clean");

        let shell_cwd = plan(
            "clean build",
            vec![ResolvedStep::ShellRun {
                cmd: "cargo clean".to_string(),
                cwd: Some("~/github/jisuno".to_string()),
            }],
        );
        assert_eq!(
            AppState::confirm_summary(&shell_cwd),
            "cargo clean (in ~/github/jisuno)"
        );

        let multi = plan(
            "tidy",
            vec![
                ResolvedStep::ShellRun {
                    cmd: "cargo clean".to_string(),
                    cwd: None,
                },
                ResolvedStep::ThemeSet {
                    theme: "dark".to_string(),
                },
            ],
        );
        assert_eq!(
            AppState::confirm_summary(&multi),
            "shell.run cargo clean, theme.set dark"
        );
    }
}
