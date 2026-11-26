//! Embedded defaults and schemas stay consistent with the modules that read them.

use std::fs;
use std::path::PathBuf;

use jisuno_logic::{keybindings, settings::Settings};

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn settings_defaults_parse_to_settings_default() {
    let text = include_str!("../defaults/settings.json");
    let parsed = Settings::parse(text).expect("defaults/settings.json must validate");
    assert_eq!(parsed, Settings::default());
}

#[test]
fn keybindings_defaults_have_no_diagnostics() {
    let text = include_str!("../defaults/keybindings.json");
    let (bindings, warnings) =
        keybindings::parse_with_warnings(text).expect("defaults/keybindings.json must validate");
    assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    assert_eq!(bindings, keybindings::defaults());
}

#[test]
fn every_default_file_has_a_matching_schema() {
    let defaults_dir = crate_dir().join("defaults");
    let mut checked = 0;
    for entry in fs::read_dir(&defaults_dir).expect("read defaults/") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("utf-8 file stem")
            .to_string();
        let text = fs::read_to_string(&path).expect("read default file");
        let value: serde_json::Value =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{stem}.json: {e}"));
        let schema_ref = value
            .get("$schema")
            .and_then(|s| s.as_str())
            .unwrap_or_else(|| panic!("{stem}.json: missing \"$schema\""));
        let expected = format!("./schemas/{stem}.schema.json");
        assert_eq!(schema_ref, expected, "{stem}.json: wrong $schema");
        assert!(
            crate_dir().join(&expected).is_file(),
            "{stem}.json: {expected} does not exist"
        );
        checked += 1;
    }
    assert!(
        checked >= 3,
        "expected at least 3 default files, found {checked}"
    );
}
