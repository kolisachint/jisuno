//! Tests: defaults, the §2 example, one test per §7 row, line numbers,
//! and round-trip stability.

use super::*;

fn errors(text: &str) -> Vec<Diagnostic> {
    match parse(text) {
        Ok((_, warnings)) => panic!("expected errors, got warnings: {warnings:?}"),
        Err(errors) => errors,
    }
}

fn ok(text: &str) -> (Config, Vec<Diagnostic>) {
    parse(text).unwrap_or_else(|errors| panic!("expected ok, got: {errors:?}"))
}

fn doc(commands: &str) -> String {
    format!("{{\"version\": 1, \"commands\": [{commands}], \"workspaces\": {{}}}}")
}

fn menu_command(id: &str, say: &str) -> String {
    format!("{{\"id\": \"{id}\", \"say\": [\"{say}\"], \"steps\": [{{\"do\": \"jisuno.menu\"}}]}}")
}

#[test]
fn defaults_are_clean() {
    let config = defaults().expect("embedded defaults parse");
    assert_eq!(config.commands.len(), 5);
    let (_, warnings) = ok(include_str!("../../defaults/voice-control-config.json"));
    assert!(warnings.is_empty());
}

#[test]
fn section_two_example_is_clean() {
    let text = r#"{
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
    {"id": "send", "say": ["send it"], "steps": [{"do": "ax.press", "app": "focused", "role": "AXButton", "name": "Send"}]},
    {"id": "clean", "say": ["clean build"], "confirm": true, "steps": [{"do": "shell.run", "cmd": "cargo clean"}]},
    {"id": "dark", "say": ["dark mode"], "steps": [{"do": "theme.set", "theme": "quiet-dark"}]},
    {"id": "light", "say": ["light mode"], "steps": [{"do": "theme.set", "theme": "quiet-light"}]},
    {"id": "reply", "say": ["reply"], "when": {"app": ["com.apple.mail"]},
     "steps": [{"do": "ax.press", "app": "focused", "role": "AXButton", "name": "Reply"}]},
    {"id": "project", "say": ["open project {project}"],
     "slots": {"project": ["jisuno", "inkuno", "rex"]},
     "steps": [{"do": "app.open", "app": "Zed", "arg": "~/github/{project}"}]}
  ],
  "workspaces": {
    "work": [
      {"id": "terminal", "app": "Rex", "title": "hoocode — ~/github/jisuno", "run": "hoocode", "place": "left-half"},
      {"id": "editor", "app": "Zed", "title": "jisuno — Zed", "open": "~/github/jisuno", "place": "right-top"},
      {"id": "browser", "app": "Safari", "title": "localhost:4000", "open": "http://localhost:4000", "place": "right-bottom"}
    ]
  }
}"#;
    let (config, warnings) = ok(text);
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    assert_eq!(config.commands.len(), 13);
    assert_eq!(config.workspaces["work"].len(), 3);
}

#[test]
fn row_01_invalid_json() {
    let diags = errors("{invalid");
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].line, 1);
    assert_eq!(diags[0].severity, Severity::Error);
    assert!(
        diags[0].message.starts_with("invalid JSON: "),
        "{}",
        diags[0].message
    );
}

#[test]
fn row_02_unknown_keys() {
    let text = doc(
        r#"{"id": "clean", "say": ["clean build"], "confirmm": true, "steps": [{"do": "jisuno.menu"}]}"#,
    );
    let diags = errors(&text);
    assert_eq!(diags.len(), 1);
    assert_eq!(
        diags[0].message,
        "unknown key \"confirmm\" in command \"clean\""
    );

    let diags = errors(r#"{"version": 1, "commands": [], "bogus": 1}"#);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].message, "unknown key \"bogus\"");

    let text = doc(
        r#"{"id": "a", "say": ["x"], "steps": [{"do": "jisuno.menu"}], "when": {"app": ["com.a.B"], "foo": 1}}"#,
    );
    let diags = errors(&text);
    assert_eq!(diags.len(), 1);
    assert_eq!(
        diags[0].message,
        "unknown key \"foo\" in command \"a\" when"
    );

    let text = r#"{"version": 1, "commands": [], "workspaces": {"w": [{"id": "t", "app": "Rex", "place": "left-half", "zoom": 1}]}}"#;
    let diags = errors(text);
    assert_eq!(diags.len(), 1);
    assert_eq!(
        diags[0].message,
        "unknown key \"zoom\" in workspace \"w\" window \"t\""
    );
}

#[test]
fn row_03_command_missing_id() {
    let text = doc(r#"{"say": ["x"], "steps": [{"do": "jisuno.menu"}]}"#);
    let diags = errors(&text);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].message, "command 1: missing \"id\"");
}

#[test]
fn row_04_command_missing_say() {
    let text = doc(r#"{"id": "clean", "steps": [{"do": "jisuno.menu"}]}"#);
    let diags = errors(&text);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].message, "command \"clean\": missing \"say\"");
}

#[test]
fn row_05_command_missing_steps() {
    let text = doc(r#"{"id": "clean", "say": ["x"]}"#);
    let diags = errors(&text);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].message, "command \"clean\": missing \"steps\"");
}

#[test]
fn row_06_duplicate_id() {
    let text = format!(
        "{{\"version\": 1, \"commands\": [\n{}\n,\n{}\n]}}",
        menu_command("clean", "clean one"),
        menu_command("clean", "clean two")
    );
    let diags = errors(&text);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].line, 4);
    assert_eq!(diags[0].severity, Severity::Error);
    assert_eq!(diags[0].message, "duplicate id \"clean\"");
}

#[test]
fn row_07_duplicate_phrase() {
    let text = format!(
        "{{\"version\": 1, \"commands\": [\n{}\n,\n{}\n]}}",
        menu_command("settings", "open settings"),
        menu_command("setup", "open settings")
    );
    let diags = errors(&text);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].line, 4);
    assert_eq!(diags[0].severity, Severity::Error);
    assert_eq!(
        diags[0].message,
        "duplicate phrase \"open settings\" (commands \"settings\" and \"setup\")"
    );

    // Disjoint `when.app` lists may share a phrase.
    let text = format!(
        "{{\"version\": 1, \"commands\": [{}, {}]}}",
        r#"{"id": "settings", "say": ["open settings"], "when": {"app": ["a.A"]}, "steps": [{"do": "jisuno.menu"}]}"#,
        r#"{"id": "setup", "say": ["open settings"], "when": {"app": ["b.B"]}, "steps": [{"do": "jisuno.menu"}]}"#,
    );
    let (_, warnings) = ok(&text);
    assert!(warnings.is_empty());
}

#[test]
fn row_08_empty_phrase() {
    let text = doc(r#"{"id": "clean", "say": ["   "], "steps": [{"do": "jisuno.menu"}]}"#);
    let diags = errors(&text);
    assert_eq!(diags.len(), 1);
    assert_eq!(
        diags[0].message,
        "command \"clean\": empty phrase in \"say\""
    );
}

#[test]
fn row_09_unknown_action() {
    let text =
        doc(r#"{"id": "clean", "say": ["x"], "steps": [{"do": "shell.exec", "cmd": "ls"}]}"#);
    let diags = errors(&text);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].severity, Severity::Error);
    assert_eq!(
        diags[0].message,
        "command \"clean\": unknown action \"shell.exec\""
    );
}

#[test]
fn row_10_missing_argument() {
    let text = doc(
        r#"{"id": "move", "say": ["move it"], "steps": [{"do": "window.tile", "window": "t"}]}"#,
    );
    let diags = errors(&text);
    assert_eq!(diags.len(), 1);
    assert_eq!(
        diags[0].message,
        "command \"move\" step 1 (window.tile): missing argument \"place\""
    );
}

#[test]
fn row_11_unknown_argument() {
    let text = doc(
        r#"{"id": "focus", "say": ["focus it"], "steps": [{"do": "window.focus", "window": "t", "place": "left-half"}]}"#,
    );
    let diags = errors(&text);
    assert_eq!(diags.len(), 1);
    assert_eq!(
        diags[0].message,
        "command \"focus\" step 1 (window.focus): unknown argument \"place\""
    );
}

#[test]
fn row_12_slot_in_steps_missing_from_phrases() {
    let text = doc(
        r#"{"id": "move", "say": ["move it"], "steps": [{"do": "window.focus", "window": "{window}"}]}"#,
    );
    let diags = errors(&text);
    assert_eq!(diags.len(), 1);
    assert_eq!(
        diags[0].message,
        "command \"move\": slot \"{window}\" used in steps but not in any phrase"
    );
}

#[test]
fn row_13_phrase_slot_without_list() {
    let text = doc(
        r#"{"id": "note", "say": ["open note {notebook}"], "steps": [{"do": "app.open", "app": "Inkuno", "arg": "{notebook}.md"}]}"#,
    );
    let diags = errors(&text);
    assert_eq!(diags.len(), 1);
    assert_eq!(
        diags[0].message,
        "command \"note\": slot \"{notebook}\" in phrase \"open note {notebook}\" has no list or declaration"
    );
}

#[test]
fn row_14_free_slot_rules() {
    let text = doc(
        r#"{"id": "note", "say": ["open {name} {name}"], "steps": [{"do": "app.open", "app": "Inkuno"}]}"#,
    );
    let diags = errors(&text);
    assert_eq!(diags.len(), 1);
    assert_eq!(
        diags[0].message,
        "command \"note\": free slot must be the only free slot and the last token: \"open {name} {name}\""
    );

    let text = doc(
        r#"{"id": "note", "say": ["note {name} now"], "steps": [{"do": "app.open", "app": "Inkuno"}]}"#,
    );
    let diags = errors(&text);
    assert_eq!(diags.len(), 1);
    assert_eq!(
        diags[0].message,
        "command \"note\": free slot must be the only free slot and the last token: \"note {name} now\""
    );
}

#[test]
fn row_15_shell_run_needs_confirm() {
    let text = doc(
        r#"{"id": "clean", "say": ["clean build"], "steps": [{"do": "shell.run", "cmd": "cargo clean"}]}"#,
    );
    let diags = errors(&text);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].severity, Severity::Error);
    assert_eq!(
        diags[0].message,
        "command \"clean\": shell.run requires \"confirm\": true"
    );
}

#[test]
fn row_16_unknown_workspace() {
    let text = doc(
        r#"{"id": "work.start", "say": ["start working"], "steps": [{"do": "workspace.open", "workspace": "wrk"}]}"#,
    );
    let diags = errors(&text);
    assert_eq!(diags.len(), 1);
    assert_eq!(
        diags[0].message,
        "command \"work.start\": unknown workspace \"wrk\""
    );
}

#[test]
fn row_17_bad_place() {
    let text = doc(
        r#"{"id": "move", "say": ["move it"], "steps": [{"do": "window.tile", "window": "t", "place": "left"}]}"#,
    );
    let diags = errors(&text);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].message, "command \"move\": bad place \"left\"");
}

#[test]
fn row_18_wait_exactly_one() {
    let text = doc(
        r#"{"id": "hold", "say": ["hold on"], "steps": [{"do": "wait", "window": "t", "ms": 5}]}"#,
    );
    let diags = errors(&text);
    assert_eq!(diags.len(), 1);
    assert_eq!(
        diags[0].message,
        "command \"hold\" step 1 (wait): exactly one of \"window\" or \"ms\""
    );

    let text = doc(r#"{"id": "hold", "say": ["hold on"], "steps": [{"do": "wait"}]}"#);
    let diags = errors(&text);
    assert_eq!(diags.len(), 1);
    assert_eq!(
        diags[0].message,
        "command \"hold\" step 1 (wait): exactly one of \"window\" or \"ms\""
    );
}

#[test]
fn row_19_wait_ms_range() {
    let text = doc(r#"{"id": "hold", "say": ["hold on"], "steps": [{"do": "wait", "ms": 20000}]}"#);
    let diags = errors(&text);
    assert_eq!(diags.len(), 1);
    assert_eq!(
        diags[0].message,
        "command \"hold\" step 1 (wait): \"ms\" must be <= 10000"
    );
}

#[test]
fn row_20_bad_bundle_id() {
    let text = doc(
        r#"{"id": "reply", "say": ["reply"], "when": {"app": ["mail"]}, "steps": [{"do": "jisuno.menu"}]}"#,
    );
    let diags = errors(&text);
    assert_eq!(diags.len(), 1);
    assert_eq!(
        diags[0].message,
        "command \"reply\": bad bundle id \"mail\""
    );
}

#[test]
fn row_21_near_duplicate_warning() {
    let text = format!(
        "{{\"version\": 1, \"commands\": [\n{}\n,\n{}\n]}}",
        menu_command("settings", "open settings"),
        menu_command("setup", "open setting")
    );
    let (_, warnings) = ok(&text);
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].line, 4);
    assert_eq!(warnings[0].severity, Severity::Warning);
    assert_eq!(
        warnings[0].message,
        "phrase \"open setting\" is very close to \"open settings\" (commands \"setup\" and \"settings\")"
    );
}

#[test]
fn line_numbers_skip_braces_and_escapes_in_strings() {
    let text = "{\n\"version\": 1,\n\"commands\": [\n{\"id\": \"q\", \"say\": [\"weird { brace \\\" quote\"], \"steps\": [{\"do\": \"nope\"}]},\n{\"id\": \"q\", \"say\": [\"other\"], \"steps\": [{\"do\": \"jisuno.menu\"}]}\n]}";
    let diags = errors(text);
    assert_eq!(diags.len(), 2);
    assert_eq!(diags[0].line, 4);
    assert_eq!(diags[0].message, "command \"q\": unknown action \"nope\"");
    assert_eq!(diags[1].line, 5);
    assert_eq!(diags[1].message, "duplicate id \"q\"");
}

#[test]
fn diagnostic_render_format() {
    let diagnostic = Diagnostic {
        line: 14,
        severity: Severity::Error,
        message: "duplicate id \"clean\"".to_string(),
    };
    assert_eq!(
        diagnostic.render("voice-control-config.json"),
        "voice-control-config.json:14: duplicate id \"clean\""
    );
}

#[test]
fn normalize_rules() {
    assert_eq!(normalize("Open, Settings!"), vec!["open", "settings"]);
    assert_eq!(
        normalize("move {Window} to {Place}"),
        vec!["move", "{window}", "to", "{place}"]
    );
    assert!(normalize("...").is_empty());
}

#[test]
fn pretty_json_omits_defaults_and_round_trips() {
    let config = defaults().expect("embedded defaults parse");
    let pretty = config.to_pretty_json();
    assert!(!pretty.contains("confirm"));
    assert!(!pretty.contains("agent"));
    assert!(!pretty.contains("slots"));
    assert!(!pretty.contains("\"when\""));

    let (again, warnings) = ok(&pretty);
    assert!(warnings.is_empty());
    assert_eq!(again, config);
    assert_eq!(again.to_pretty_json(), pretty);
}

#[test]
fn pretty_json_matches_defaults_file_byte_for_byte() {
    let config = defaults().expect("embedded defaults parse");
    assert_eq!(
        config.schema.as_deref(),
        Some("./schemas/voice-control-config.schema.json")
    );
    assert_eq!(
        config.to_pretty_json(),
        include_str!("../../defaults/voice-control-config.json")
    );
}

#[test]
fn add_command_validates_result() {
    let mut config = defaults().expect("embedded defaults parse");
    let len = config.commands.len();
    config
        .add_command(CommandDef {
            id: "mail".to_string(),
            say: vec!["open mail".to_string()],
            steps: vec![Step::AppOpen {
                app: "Mail".to_string(),
                arg: None,
            }],
            confirm: false,
            when: None,
            slots: BTreeMap::new(),
            agent: true,
        })
        .expect("valid command adds cleanly");
    assert_eq!(config.commands.len(), len + 1);
    assert!(config.commands.last().is_some_and(|c| c.agent));
    let (again, warnings) = ok(&config.to_pretty_json());
    assert!(warnings.is_empty());
    assert_eq!(again, config);

    let before = config.clone();
    let errors = config
        .add_command(CommandDef {
            id: "menu".to_string(),
            say: vec!["open mail".to_string()],
            steps: vec![Step::JisunoMenu],
            confirm: false,
            when: None,
            slots: BTreeMap::new(),
            agent: true,
        })
        .expect_err("duplicate id must fail");
    assert!(errors.iter().any(|e| e.severity == Severity::Error));
    assert_eq!(config, before, "failed add leaves config unchanged");
}

#[test]
fn remove_command_reports_presence() {
    let mut config = defaults().expect("embedded defaults parse");
    assert!(config.remove_command("dark"));
    assert!(!config.commands.iter().any(|c| c.id == "dark"));
    assert!(!config.remove_command("dark"));
    let (again, warnings) = ok(&config.to_pretty_json());
    assert!(warnings.is_empty());
    assert_eq!(again, config);
}

#[test]
fn non_ascii_keys_map_to_the_right_line() {
    let text = "{\n\"version\": 1,\n\"workspaces\": {\"caf\u{e9}\": [\n{\"id\": \"t\", \"app\": \"Rex\", \"place\": \"left-half\", \"zoom\": 1}\n]},\n\"commands\": []}";
    let diags = errors(text);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].line, 4);
    assert_eq!(
        diags[0].message,
        "unknown key \"zoom\" in workspace \"caf\u{e9}\" window \"t\""
    );
}
