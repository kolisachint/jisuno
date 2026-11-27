//! Config files embedded in this crate: the defaults and JSON Schemas written on first run.
//!
//! The files live under `defaults/` and `schemas/` next to this crate's `Cargo.toml`, so they
//! ship with the published crate. Other crates must read them through this module, not with
//! `include_str!` on a path outside the crate (that breaks `cargo package`).

use crate::settings::EmbeddedFile;

pub const SETTINGS_DEFAULTS: &str = include_str!("../defaults/settings.json");
pub const SETTINGS_SCHEMA: &str = include_str!("../schemas/settings.schema.json");
pub const KEYBINDINGS_DEFAULTS: &str = include_str!("../defaults/keybindings.json");
pub const KEYBINDINGS_SCHEMA: &str = include_str!("../schemas/keybindings.schema.json");
pub const THEME_DEFAULTS: &str = include_str!("../defaults/theme.json");
pub const THEME_SCHEMA: &str = include_str!("../schemas/theme.schema.json");
pub const VOICE_CONTROL_DEFAULTS: &str = include_str!("../defaults/voice-control-config.json");
pub const VOICE_CONTROL_SCHEMA: &str = include_str!("../schemas/voice-control-config.schema.json");

/// Every embedded file with its name under the config dir, in first-run write order.
pub fn first_run_files() -> Vec<EmbeddedFile> {
    vec![
        EmbeddedFile {
            name: "settings.json",
            contents: SETTINGS_DEFAULTS,
        },
        EmbeddedFile {
            name: "schemas/settings.schema.json",
            contents: SETTINGS_SCHEMA,
        },
        EmbeddedFile {
            name: "keybindings.json",
            contents: KEYBINDINGS_DEFAULTS,
        },
        EmbeddedFile {
            name: "schemas/keybindings.schema.json",
            contents: KEYBINDINGS_SCHEMA,
        },
        EmbeddedFile {
            name: "theme.json",
            contents: THEME_DEFAULTS,
        },
        EmbeddedFile {
            name: "schemas/theme.schema.json",
            contents: THEME_SCHEMA,
        },
        EmbeddedFile {
            name: "voice-control-config.json",
            contents: VOICE_CONTROL_DEFAULTS,
        },
        EmbeddedFile {
            name: "schemas/voice-control-config.schema.json",
            contents: VOICE_CONTROL_SCHEMA,
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn first_run_files_are_unique_and_valid_json() {
        let files = first_run_files();
        let names: HashSet<_> = files.iter().map(|f| f.name).collect();
        assert_eq!(names.len(), files.len());
        for file in files {
            serde_json::from_str::<serde_json::Value>(file.contents)
                .unwrap_or_else(|e| panic!("{} is not JSON: {e}", file.name));
        }
    }
}
