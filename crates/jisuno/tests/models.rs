//! Integration tests for `jisuno models` (`download`/`remove`/`verify`/`list`).
//! No real network: downloads are never attempted; size checks use sparse
//! files and the CLI runs with `JISUNO_MODELS_DIR` pointed at a tempdir.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use jisuno_asr::setup::Model;

fn jisuno(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_jisuno"))
        .arg("models")
        .args(args)
        .env("JISUNO_MODELS_DIR", dir)
        .output()
        .expect("run jisuno models")
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

fn install_sparse(dir: &Path, model: &Model) {
    let dest = dir.join(model.id());
    fs::create_dir_all(&dest).expect("mkdir model dir");
    for spec in model.specs() {
        let f = fs::File::create(dest.join(spec.local_name)).expect("create file");
        f.set_len(spec.size).expect("sparse size");
    }
}

#[test]
fn list_shows_both_models_not_installed() {
    let tmp = tempfile::tempdir().unwrap();
    let out = jisuno(tmp.path(), &[]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    let body = stdout(&out);
    let v3 = Model::ParakeetV3Int8.id();
    let v2 = Model::ParakeetV2Int8.id();
    assert!(body.contains(v3), "lists v3: {body}");
    assert!(body.contains(v2), "lists v2: {body}");
    assert!(body.contains("not installed"), "fresh dir: {body}");
    let v3_line = body.lines().find(|l| l.contains(v3)).expect("v3 line");
    assert!(v3_line.contains("(default)"), "default marker: {v3_line}");
    let v2_line = body.lines().find(|l| l.contains(v2)).expect("v2 line");
    assert!(!v2_line.contains("(default)"), "v2 not default: {v2_line}");
}

#[test]
fn remove_and_verify_missing_exit_1() {
    let tmp = tempfile::tempdir().unwrap();
    let out = jisuno(tmp.path(), &["remove", Model::ParakeetV3Int8.id()]);
    assert_eq!(code(&out), 1, "stdout: {}", stdout(&out));
    assert!(stdout(&out).contains("not installed"), "{}", stdout(&out));

    let out = jisuno(tmp.path(), &["verify", Model::ParakeetV2Int8.id()]);
    assert_eq!(code(&out), 1);

    let out = jisuno(tmp.path(), &["remove", "bogus-model"]);
    assert_eq!(code(&out), 1);
    assert!(stdout(&out).contains("unknown model"), "{}", stdout(&out));
}

#[test]
fn sparse_sized_files_count_as_installed_but_fail_verify() {
    let tmp = tempfile::tempdir().unwrap();
    let model = Model::ParakeetV3Int8;
    install_sparse(tmp.path(), &model);

    // Size check says installed...
    let out = jisuno(tmp.path(), &[]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    let line = stdout(&out)
        .lines()
        .find(|l| l.contains(model.id()))
        .expect("v3 line")
        .to_owned();
    assert!(line.contains("[installed]"), "{line}");

    // ...but the hashes (all zeros) do not verify.
    let out = jisuno(tmp.path(), &["verify", model.id()]);
    assert_eq!(code(&out), 1, "stdout: {}", stdout(&out));
    assert!(stdout(&out).contains("sha256 mismatch"), "{}", stdout(&out));

    // Removal works and the dir is gone afterwards.
    let out = jisuno(tmp.path(), &["remove", model.id()]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("removed"), "{}", stdout(&out));
    assert!(!tmp.path().join(model.id()).exists());
}

static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn remove_refuses_while_download_lock_held() {
    let _env = ENV_GUARD.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("JISUNO_MODELS_DIR", tmp.path());
    let model = Model::ParakeetV3Int8;
    // Hold the lock in-process (same pid the CLI will see as live).
    let _guard = jisuno_asr::setup::acquire_lock(&model).expect("acquire");
    assert!(jisuno_asr::setup::acquire_lock(&model).is_err(), "live lock refuses");
    let out = jisuno(tmp.path(), &["remove", model.id()]);
    std::env::remove_var("JISUNO_MODELS_DIR");
    assert_eq!(code(&out), 1, "stdout: {}", stdout(&out));
    assert!(stdout(&out).contains("downloading"), "{}", stdout(&out));
}

#[test]
fn models_root_override_is_used() {
    let tmp = tempfile::tempdir().unwrap();
    let out = jisuno(tmp.path(), &[]);
    assert_eq!(code(&out), 0);
    assert!(
        stderr(&out).contains(&tmp.path().display().to_string()),
        "points at override: {}",
        stderr(&out)
    );
    // Nothing was created by a read-only list.
    let entries: Vec<PathBuf> = fs::read_dir(tmp.path()).unwrap().map(|e| e.unwrap().path()).collect();
    assert!(entries.is_empty(), "{entries:?}");
}
