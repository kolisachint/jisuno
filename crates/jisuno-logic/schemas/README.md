JSON Schemas for the four ~/.config/jisuno/ config files, plus the first-run defaults.
Embedded into the binary and written to ~/.config/jisuno/schemas/ (defaults to ~/.config/jisuno/) on first run, so editors complete and check the files with no network.
Hot-reloaded: an invalid file is rejected and the last good config stays live.
Missing settings keys fall back to their defaults.
