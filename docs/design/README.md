# docs/design: index

Design ground truth for Jisuno. Code follows these docs, never the reverse.
Status: all docs signed off 2026-11-26 (round 6): `jisuno.md`, `keybindings.md`, `voice-control.md`,
`app-ui.md` and `prototypes/v3.html`. The UI build may start.

- **`jisuno.md`**: the proposal: product, repo and crate layout, config files, voice control,
  speed and memory budget, model lifecycle, keys, permissions. Decisions log in §10 (rounds
  1–6); a decision there wins over any older text in any doc.
- **`keybindings.md`**: default chord table, hold semantics (hold-alone rule, first hold key
  wins, 2 min cap), contexts, key spelling, reserved keys, validation errors.
- **`voice-control.md`**: `voice-control-config.json`: JSON Schema, actions and their
  arguments, slot rules and matching, running and failure, validation errors, the
  `jisuno control` CLI for agents.
- **`app-ui.md`**: visual and behavior spec: tokens (inkuno Quiet plus Jisuno's two accents,
  macOS system fonts), menu bar icon and badges, HUD, popover, Settings window, first-run Welcome
  window, error states. `theme.json` schema and defaults: `crates/jisuno-logic/schemas/theme.schema.json`,
  `crates/jisuno-logic/defaults/theme.json`.
- **`prototypes/`**: clickable HTML prototypes, one self-contained file per version; v3 is
  current. See `prototypes/README.md`.
