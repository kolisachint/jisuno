# Prototypes and design log

Clickable HTML prototypes of Jisuno, made to agree on the design before it is built. Each version
is a single self-contained file: open it in a browser. A version is never edited once a newer one
exists; changes go into the next version and are logged here.

Prototype limits: speech is scripted (pick the phrase you "say"), latency numbers are simulated
from the budget in `../jisuno.md` §5, the desktop and its windows are fakes.

| Version | Date | File |
|--|--|--|
| v1 | 2026-11-26 | `v1.html` |
| v2 | 2026-11-26 | `v2.html` |

## v2: owner decisions applied (2026-11-26)

Changes from v1, following `../jisuno.md` §10:

- Command hold key is Right ⌘ (was Right ⌥+⇧). New warm key ⌃⌥W.
- Model lifecycle: starts unloaded; a hold key while cold loads under your speech (load bar in
  the HUD, partials begin once warm); idle unload after `unload_after_idle_min` (prototype
  time runs 60x; "Skip idle" button). Icon dims when unloaded, pulses while loading.
- `voice-control-config.json` is the fourth file and owns commands and workspaces. Commands
  take `{slots}` ("move terminal to right half", "open note standup") and run `steps`.
- Confirm flow: "clean build" (`shell.run`) waits for Return.
- Agent terminal in the review panel: `jisuno control list|add|remove|validate` and
  `jisuno model status|warm|unload`. Validation rejects `shell.run` without confirm.
- Settings pages: Model, Voice control, Performance (targets, empty measured column,
  simulated session p50/p95).

## v1: first proposal (2026-11-26)

- Menu bar icon with states (idle, dictating in blue, command in amber) and a popover: model
  status, the three voice actions with their chords, workspaces, recent results, Settings.
- HUD under the notch (or at the cursor, or off): mode, level bars, live partial text, then a
  one-line result with the latency.
- Dictation inserts into the focused window (a fake TextEdit).
- Commands: fuzzy-matched fixed phrases. "start working" opens and tiles three windows (Rex,
  Zed, Safari) from `settings.json` → `workspaces.work`.
- Settings window: every control edits one key, shown highlighted in the live JSON of
  `settings.json`, `keybindings.json` or `theme.json`. Keybindings are recordable.
- Keys: hold Right ⌥ dictate, hold Right ⌥ + ⇧ command, ⌃⌥Space hands-free, Esc cancel,
  ⌃⌥, Settings.
