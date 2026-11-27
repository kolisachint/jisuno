//! `jisuno model ...` without a running app: exit 1 + the not-running line.
//! HOME points at a temp dir so no real socket can exist.

#[test]
fn model_status_without_app_exits_1() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = env!("CARGO_BIN_EXE_jisuno");
    let output = std::process::Command::new(bin)
        .args(["model", "status"])
        .env("HOME", tmp.path())
        .output()
        .expect("run jisuno model status");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error: Jisuno is not running"),
        "stderr: {stderr}"
    );
}

#[test]
fn model_warm_without_app_exits_1() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let bin = env!("CARGO_BIN_EXE_jisuno");
    let output = std::process::Command::new(bin)
        .args(["model", "warm"])
        .env("HOME", tmp.path())
        .output()
        .expect("run jisuno model warm");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error: Jisuno is not running"),
        "stderr: {stderr}"
    );
}
