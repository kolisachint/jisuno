//! Integration tests for `jisuno control` (`docs/design/voice-control.md` §8).
//! Each test runs the built binary with `JISUNO_CONFIG_DIR` set to a tempdir.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SCHEMA_TEXT: &str =
    include_str!("../../jisuno-logic/schemas/voice-control-config.schema.json");

/// §8 starting point: `menu` and `settings` only.
const SEED: &str = r#"{
  "version": 1,
  "commands": [
    {"id": "menu", "say": ["open jisuno"], "steps": [{"do": "jisuno.menu"}]},
    {"id": "settings", "say": ["open settings"], "steps": [{"do": "jisuno.settings"}]}
  ],
  "workspaces": {}
}
"#;

fn config_path(dir: &Path) -> PathBuf {
    dir.join("voice-control-config.json")
}

fn write_seed(dir: &Path, text: &str) {
    fs::write(config_path(dir), text).expect("write seed config");
}

fn control(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_jisuno"))
        .arg("control")
        .args(args)
        .env("JISUNO_CONFIG_DIR", dir)
        .output()
        .expect("run jisuno control")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("utf-8 stdout")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("utf-8 stderr")
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("exit code")
}

#[test]
fn list_without_file_exits_1() {
    let tmp = tempfile::tempdir().unwrap();
    let out = control(tmp.path(), &["list"]);
    assert_eq!(code(&out), 1);
    assert_eq!(
        stderr(&out),
        format!(
            "error: no voice-control-config.json at {} (run the app once or create it)\n",
            config_path(tmp.path()).display()
        )
    );
}

#[test]
fn validate_without_file_exits_1() {
    let tmp = tempfile::tempdir().unwrap();
    let out = control(tmp.path(), &["validate"]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).starts_with("error: no voice-control-config.json at "));
}

#[test]
fn add_without_file_starts_from_defaults() {
    let tmp = tempfile::tempdir().unwrap();
    let out = control(
        tmp.path(),
        &[
            "add",
            "--id",
            "mail",
            "--say",
            "open mail",
            "--do",
            "app.open",
            "--app",
            "Mail",
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "added \"mail\"\n");

    let schema = tmp
        .path()
        .join("schemas")
        .join("voice-control-config.schema.json");
    assert_eq!(fs::read_to_string(schema).unwrap(), SCHEMA_TEXT);

    let list = control(tmp.path(), &["list"]);
    assert_eq!(code(&list), 0);
    let table = stdout(&list);
    for id in ["menu", "settings", "dark", "light", "warm", "mail"] {
        assert!(table.lines().any(|line| line.starts_with(id)), "{table}");
    }
}

#[test]
fn section_8_walkthrough() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write_seed(dir, SEED);

    let out = control(
        dir,
        &[
            "add",
            "--id",
            "mail",
            "--say",
            "open mail",
            "--do",
            "app.open",
            "--app",
            "Mail",
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "added \"mail\"\n");

    let out = control(
        dir,
        &[
            "add",
            "--id",
            "clean",
            "--say",
            "clean build",
            "--do",
            "shell.run",
            "--cmd",
            "cargo clean",
            "--confirm",
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "added \"clean\"\n");

    let out = control(dir, &["list"]);
    assert_eq!(code(&out), 0);
    assert_eq!(
        stdout(&out),
        "id          phrases         steps\n\
         menu        open jisuno     jisuno.menu\n\
         settings    open settings   jisuno.settings\n\
         mail        open mail       app.open Mail\n\
         clean       clean build     shell.run cargo clean\n"
    );

    let out = control(dir, &["list", "--json"]);
    assert_eq!(code(&out), 0);
    let commands: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let mail = commands
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "mail")
        .expect("mail in list --json");
    assert_eq!(mail["say"], serde_json::json!(["open mail"]));
    assert_eq!(
        mail["steps"],
        serde_json::json!([{"do": "app.open", "app": "Mail"}])
    );
    assert_eq!(mail["agent"], serde_json::json!(true));

    let out = control(dir, &["validate"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "ok\n");

    let before = fs::read(config_path(dir)).unwrap();
    let out = control(
        dir,
        &[
            "add",
            "--id",
            "bad",
            "--say",
            "bad thing",
            "--do",
            "shell.run",
            "--cmd",
            "rm -rf /",
        ],
    );
    assert_eq!(code(&out), 2);
    assert_eq!(stdout(&out), "");
    assert_eq!(
        stderr(&out),
        "error: command \"bad\": shell.run requires \"confirm\": true\n"
    );
    assert_eq!(
        fs::read(config_path(dir)).unwrap(),
        before,
        "file changed on rejected add"
    );

    let out = control(dir, &["remove", "mail"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "removed \"mail\"\n");
}

#[test]
fn list_shows_steps_summary_per_action() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write_seed(dir, SEED);
    let out = control(
        dir,
        &[
            "add",
            "--id",
            "dark",
            "--say",
            "dark mode",
            "--do",
            "theme.set",
            "--theme",
            "quiet-dark",
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = control(dir, &["list"]);
    assert!(
        stdout(&out).contains("dark        dark mode       theme.set quiet-dark\n"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn add_with_two_phrases_and_when_app() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write_seed(dir, SEED);
    let out = control(
        dir,
        &[
            "add",
            "--id",
            "reply",
            "--say",
            "reply",
            "--say",
            "reply to this",
            "--do",
            "ax.press",
            "--app",
            "focused",
            "--role",
            "AXButton",
            "--name",
            "Reply",
            "--when-app",
            "com.apple.mail",
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let out = control(dir, &["list", "--json"]);
    let commands: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let reply = commands
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "reply")
        .unwrap();
    assert_eq!(reply["say"], serde_json::json!(["reply", "reply to this"]));
    assert_eq!(
        reply["when"],
        serde_json::json!({"app": ["com.apple.mail"]})
    );
    assert_eq!(
        reply["steps"],
        serde_json::json!([{"do": "ax.press", "app": "focused", "role": "AXButton", "name": "Reply"}])
    );
}

#[test]
fn duplicate_id_exits_2_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write_seed(dir, SEED);
    let before = fs::read(config_path(dir)).unwrap();
    let out = control(
        dir,
        &[
            "add",
            "--id",
            "menu",
            "--say",
            "other menu",
            "--do",
            "jisuno.menu",
        ],
    );
    assert_eq!(code(&out), 2);
    assert_eq!(stderr(&out), "error: duplicate id \"menu\"\n");
    assert_eq!(fs::read(config_path(dir)).unwrap(), before);
}

#[test]
fn add_missing_argument_exits_2() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write_seed(dir, SEED);
    let out = control(
        dir,
        &[
            "add",
            "--id",
            "mv",
            "--say",
            "move it",
            "--do",
            "window.tile",
            "--window",
            "terminal",
        ],
    );
    assert_eq!(code(&out), 2);
    assert_eq!(
        stderr(&out),
        "error: command \"mv\" step 1 (window.tile): missing argument \"place\"\n"
    );
}

#[test]
fn add_unknown_action_exits_2() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write_seed(dir, SEED);
    let out = control(
        dir,
        &["add", "--id", "x", "--say", "x y", "--do", "shell.exec"],
    );
    assert_eq!(code(&out), 2);
    assert_eq!(
        stderr(&out),
        "error: command \"x\": unknown action \"shell.exec\"\n"
    );
}

#[test]
fn add_irrelevant_argument_exits_1() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write_seed(dir, SEED);
    let before = fs::read(config_path(dir)).unwrap();
    let out = control(
        dir,
        &[
            "add",
            "--id",
            "focus",
            "--say",
            "focus mail",
            "--do",
            "window.focus",
            "--window",
            "Mail",
            "--place",
            "left-half",
        ],
    );
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("--place"), "{}", stderr(&out));
    assert_eq!(fs::read(config_path(dir)).unwrap(), before);
}

#[test]
fn remove_unknown_id_exits_1() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write_seed(dir, SEED);
    let before = fs::read(config_path(dir)).unwrap();
    let out = control(dir, &["remove", "nope"]);
    assert_eq!(code(&out), 1);
    assert_eq!(stderr(&out), "error: no command \"nope\"\n");
    assert_eq!(fs::read(config_path(dir)).unwrap(), before);
}

#[test]
fn remove_leaves_no_temp_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write_seed(dir, SEED);
    let out = control(dir, &["remove", "menu"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["voice-control-config.json".to_string()]);
}

#[test]
fn validate_broken_file_exits_2_with_location() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write_seed(
        dir,
        "{\n  \"version\": 1,\n  \"commands\": [\n    {\"id\": \"clean\", \"say\": [\"clean build\"], \"steps\": [{\"do\": \"shell.run\", \"cmd\": \"cargo clean\"}]}\n  ]\n}\n",
    );
    let out = control(dir, &["validate"]);
    assert_eq!(code(&out), 2);
    assert_eq!(
        stderr(&out),
        "voice-control-config.json:4: command \"clean\": shell.run requires \"confirm\": true\n"
    );
    assert_eq!(stdout(&out), "");
}

#[test]
fn validate_invalid_json_exits_2() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write_seed(dir, "{\n  \"version\": 1,\n");
    let out = control(dir, &["validate"]);
    assert_eq!(code(&out), 2);
    assert!(
        stderr(&out).starts_with("voice-control-config.json:"),
        "{}",
        stderr(&out)
    );
    assert!(stderr(&out).contains("invalid JSON: "), "{}", stderr(&out));
}

#[test]
fn add_to_broken_file_exits_2_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let broken = "{\n  \"version\": 1,\n  \"commands\": [\n    {\"id\": \"clean\", \"say\": [\"clean build\"], \"steps\": [{\"do\": \"shell.run\", \"cmd\": \"cargo clean\"}]}\n  ]\n}\n";
    write_seed(dir, broken);
    let out = control(
        dir,
        &[
            "add",
            "--id",
            "menu",
            "--say",
            "open jisuno",
            "--do",
            "jisuno.menu",
        ],
    );
    assert_eq!(code(&out), 2);
    assert_eq!(
        stderr(&out),
        "voice-control-config.json:4: command \"clean\": shell.run requires \"confirm\": true\n"
    );
    assert_eq!(fs::read_to_string(config_path(dir)).unwrap(), broken);
}

#[test]
fn validate_prints_warnings_and_exits_0() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write_seed(
        dir,
        "{\n  \"version\": 1,\n  \"commands\": [\n    {\"id\": \"settings\", \"say\": [\"open settings\"], \"steps\": [{\"do\": \"jisuno.settings\"}]},\n    {\"id\": \"setup\", \"say\": [\"open setting\"], \"steps\": [{\"do\": \"jisuno.menu\"}]}\n  ]\n}\n",
    );
    let out = control(dir, &["validate"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "ok\n");
    assert!(
        stderr(&out).contains("warning: voice-control-config.json:5: phrase \"open setting\" is very close to \"open settings\""),
        "{}",
        stderr(&out)
    );
}
