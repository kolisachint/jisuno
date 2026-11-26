# Prototypes and design log

Clickable HTML prototypes of Jisuno, made to agree on the design before it is built. Each version
is a single self-contained file: open it in a browser. A version is never edited once a newer one
exists; changes go into the next version and are logged here.

Prototype limits: speech is scripted (pick the phrase you "say"), latency numbers are simulated
from the budget in `../jisuno.md` §5, the desktop and its windows are fakes.

| Version | Date | File | Status |
|--|--|--|--|
| v1 | 2026-11-26 | `v1.html` | superseded by v2 |
| v2 | 2026-11-26 | `v2.html` | superseded by v3 |
| v3 | 2026-11-26 | `v3.html` | current, signed off 2026-11-26; spec in `../app-ui.md` |

## v3: app UI spec and full clickable flow (2026-11-26)

Changes from v2, following `../app-ui.md` (signed off 2026-11-26; fonts changed to SF Pro / SF Mono):

- Tokens: inkuno Quiet Light and Dark (`crates/inkuno-ui/src/theme.rs`), Jisuno's two accents with
  legible text variants, and a fixed dark HUD palette. HUD colours changed from v2.
- Menu bar icon: idle, dictating, command, loading, error; lock and warning badges.
- HUD: every state in `app-ui.md` §3.3 from a state switcher; hold keys follow the 150 ms minimum,
  tap rule and 2 min countdown (`0:15 left`); confirm prompt with 6 s timeout.
- Popover: model state with warm and unload, Recent (5, memory only), permissions, Secure Input holder,
  Settings, Quit. The v2 workspace starter is gone.
- Settings: General, Audio (mic picker, Bluetooth warning, missing device), Model (v3 default, v2
  optional, no whisper), Keybindings (record a key, conflict with Replace and Cancel, warnings), Commands
  (agent-added shown apart), Appearance, Performance (targets only), Files (open and copy error).
- Welcome: permissions checked live, config files written, model download with progress.
- Esc cancels a confirm prompt (`keybindings.md` §4, §5).

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
