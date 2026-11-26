# Jisuno: design (v3, signed off 2026-11-26)

Status: signed off 2026-11-26 (with `keybindings.md`, `voice-control.md`). UI code waits for
`app-ui.md` and `prototypes/v3.html` sign-off.
Last update: 2026-11-26. Decisions are logged in §10; they win over any older text.

## 1. What Jisuno is

`voicetools` becomes **Jisuno**: a macOS menu bar app, always one key away.

- **Dictate**: hold Right ⌥, speak, release; the text lands in the focused app.
- **Command**: hold Right ⌘, say a phrase from `voice-control-config.json` ("start working",
  "focus editor", "open note standup"); Jisuno runs its steps.
- **Cue, not chrome**: a HUD at the notch (or cursor, or off) shows mode, level, live partial
  text, model loading, then a one-line result. Short tones on start/stop/cancel.
- **Local and fast**: Parakeet TDT 0.6B v3 int8 (ONNX). Loads on demand, unloads when idle.
  Target ≤ 150 ms key-up → text with a warm model (§6).
- **Files are the settings**: `settings.json`, `keybindings.json`, `theme.json`,
  `voice-control-config.json`. The Settings window is a view over them.

Platforms: the app is **macOS only**. Linux is not a product target; it is the CI runner for
the platform-independent Rust core (§3).

## 2. Repo layout (inkuno shape)

```
jisuno/
  Cargo.toml                  virtual workspace, members = ["crates/*"]
  crates/
    jisuno/                   macOS app (GPUI): menu bar, HUD, settings; plus the CLI
                              (setup, models, transcribe, serve, control, model, bench)
    jisuno-logic/             pure, builds anywhere: config schemas + load/validate, command
                              matcher with {slots}, step runner (actions as data), line
                              protocol, VAD state machine, latency accounting
    jisuno-asr/               Transcriber trait, parakeet (ort), model registry + download,
                              model lifecycle (load/unload/idle)
    jisuno-audio/             resample, downmix, WAV; mic capture (cpal) behind a feature
    jisuno-platform/          macOS only: hotkey event tap, text insert, AX actions, window
                              placement, app launch, login item, permissions, status item
    jisuno-ui/                theme tokens, HUD, popover, settings widgets (GPUI)
    jisuno-bench/             bench harness and fixtures runner (also `jisuno bench`)
  docs/design/                this doc, keybindings.md, voice-control.md, app-ui.md,
                              prototypes/
  docs/user/                  mdBook: install, permissions, dictation, commands, config files
  packaging/                  .app bundle, entitlements, signing, notarization
```

| Today | Moves to |
|--|--|
| `main.rs` | `crates/jisuno/src/cli.rs` |
| `audio.rs`, `mic.rs` | `jisuno-audio` |
| `vad.rs`, `protocol.rs` | `jisuno-logic` |
| `transcribe/*`, `setup.rs`, `tls.rs` | `jisuno-asr` |
| `serve.rs` | `crates/jisuno` (daemon mode, same pipeline as the app) |
| `docs/hoocode-integration/` | `docs/user/` "Use from a TUI" |

## 3. CI

| Job | Runner | Covers |
|--|--|--|
| fmt, clippy, test | ubuntu | `jisuno-logic`, `jisuno-asr`, `jisuno-audio` (no mic), CLI parsing |
| ASR fixtures | ubuntu | decode fixed WAVs, check transcript and WER against thresholds |
| macOS | macos (Apple silicon) | `jisuno-platform`, `jisuno-ui`, app build, bundle |
| bench | macos | records numbers as an artifact; no gating on shared runners (§6) |

Rule: anything that can be pure goes in `jisuno-logic` so it is tested on Linux. macOS jobs run
only what needs macOS.

## 4. Config files

Location `~/.config/jisuno/`. Hot-reloaded; invalid files are rejected and the last good
config stays live, with the error in the HUD and in `jisuno control validate`.

| File | Owns |
|--|--|
| `settings.json` | `model`, `language`, `model_load` (on_demand/at_login), `unload_after_idle_min`, `input_device`, `dictation_limit_s`, `hands_free_silence_ms`, partials, `insert` (paste/type) + `insert_type_apps`, `restore_clipboard`, sounds, `hud` position, login item |
| `keybindings.json` | `{"bindings": [{key, hold?, when?, command}]}` |
| `theme.json` | theme name, appearance, dictate/command accents, HUD style |
| `voice-control-config.json` | `commands[]` and `workspaces{}` |

## 5. Voice control

### 5.1 Commands

```json
{"id":"move","say":["move {window} to {place}"],
 "steps":[{"do":"window.tile","window":"{window}","place":"{place}"}]}
```

- Fixed phrases, fuzzy matched (edit distance), plus `{slot}` templates. No LLM, no
  accessibility-tree traversal at runtime: the matcher only reads the config. Target < 5 ms.
- `steps` run in order. A step may name an app, window, role and title directly.
- `"confirm": true` shows the HUD prompt (Return runs, Esc or 6 s cancels). Required on any
  command with `shell.run`; validation rejects it otherwise.
- No match: HUD shows "Did you mean …" and nothing runs.
- `when: {"app": [bundle ids]}` limits a command to those frontmost apps.
- A failed step stops the command; the HUD names the step and the reason. No rollback.
- Schema, slot rules and validation errors: `voice-control.md`.

### 5.2 Actions v1

`jisuno.menu`, `jisuno.settings`, `app.open {app, arg}`, `window.focus {window}`,
`window.tile {window, place}`, `workspace.open/close {workspace}`,
`ax.press {app, role, name}`, `ax.set_value {app, role, name, value}`, `keys.send {keys}`,
`shell.run {cmd}` (confirm required), `theme.set {theme}`, `model.warm`, `model.unload`,
`wait {window|ms}`.

`ax.*` resolve one element by app + role + name at run time (a direct lookup, not a scan).

### 5.3 Agents: `jisuno control`

```
jisuno control list [--json]
jisuno control add --id mail --say "open mail" --do app.open --app Mail [--confirm]
jisuno control remove <id>
jisuno control validate
jisuno model status|warm|unload
```

The CLI validates before writing (exit 2 and nothing written on error), writes atomically, and
the app hot-reloads. A JSON Schema ships for humans editing by hand. Commands added by agents
are tagged `"agent": true` so the Settings window can show them.

## 6. Speed

Hold keys are the fast path: key-up ends the utterance, so there is no silence timeout.

| Step (warm model) | Target |
|--|--|
| Flush last audio chunk | ≤ 15 ms |
| Final decode, 5 s clip | ≤ 100 ms |
| Post-process | ≤ 2 ms |
| Insert (paste + clipboard restore) | ≤ 10 ms |
| **Key-up → text** | **≤ 150 ms** |

Levers: incremental (chunked) encoding instead of today's re-decode-everything partials in
`serve.rs`; CoreML vs CPU execution provider measured on the reference Mac; paste instead of
typing; mic opened on key-down.

### 6.1 Measurement

`jisuno bench` and the Settings → Performance page report: key-up → text (warm, cold),
warm decode for 2/5/15 s clips, cold load, memory warm/unloaded, idle CPU, and WER on
LibriSpeech test-clean plus an owner-recorded set. Shared CI runners record numbers only; the
reference Mac is where numbers are signed off.

Reference machine: MacBook Air M1 (MacBookAir10,1), 4 performance + 4 efficiency cores,
**8 GB RAM**, macOS 27.3. A warm model at ~0.9 GB is over 10% of that RAM, which is why
on-demand loading and idle unload are the default. The 150 ms target must hold on this machine.

## 7. Model lifecycle

States: `unloaded → loading → warm → (idle) unloaded`.

- `model_load: on_demand` (default): nothing in memory at login.
- **Warm key** (`⌃⌥W`, its own binding) loads the model ahead of time.
- If a hold key is pressed while unloaded, loading starts at key-down and runs under your
  speech; the HUD shows a thin load bar. Partials start once warm. Only the part of the load
  still left at key-up adds latency.
- `unload_after_idle_min` (default 10, 0 = never) unloads after no use.

## 8. Keys (macOS defaults, all in `keybindings.json`)

| Command | Key |
|--|--|
| `voice.dictate` | hold Right ⌥ |
| `voice.command` | hold Right ⌘ |
| `model.warm` | ⌃⌥W |
| `voice.hands_free_toggle` | ⌃⌥Space |
| `voice.cancel` | Esc (when listening or confirming) |
| `voice.confirm` | Return (when confirming) |
| `jisuno.settings` | ⌃⌥, |

Right-side modifiers held alone are free on a default macOS setup; Fn/Globe stays with Apple
dictation. A hold key used as part of a chord (Right ⌘+C) must pass through untouched: a hold
only counts if no other key is pressed during it. The first hold key wins: the other hold key
pressed during a hold is ignored (it neither switches mode nor breaks the hold). Full table
and reserved keys: `keybindings.md`.

## 9. Permissions

Microphone, Accessibility (insert, AX actions, window placement), Input Monitoring (event tap).
First run checks each one live and links to the right System Settings pane. First run also
writes all four config files in full (every key with its default) and offers the model
download (~650 MB). Error states (no mic, permission revoked, model missing, invalid config,
secure input, focus lost) are specified in `app-ui.md` and shown in `prototypes/v3.html`.

## 10. Decisions log

**2026-11-26, round 1**
1. Hotkey only, never a wake word.
2. `voice-control-config.json` owns commands and workspaces; humans and agents edit it.
3. Two hold keys, standard and non-conflicting, rebindable.
4. Speed is measured; the harness ships in v1.
5. Model loads on demand, unloads on idle (`settings.json`); a hotkey can pre-warm.
6. Mic follows the platform standard: open on key-down only.
7. GPUI, for response time.

**2026-11-26, round 2**
1. App is macOS only. Linux runs CI for the Rust core; macOS CI only for macOS code.
2. Commands are fixed and snappy: no accessibility-tree traversal.
3. Action vocabulary as in §5.2.
4. Agents go through the CLI (`jisuno control`); the file stays hand-editable.
5. Confirmation for dangerous commands; required for `shell.run`.
6. Phrases take `{slot}` parameters in v1.
7. macOS keys as in §8.
8. Warming has its own key, separate from the hold keys.
9. Idle unload default 10 min; measurement and CI plan as in §3 and §6.1.

**2026-11-26, round 3**
1. Hold-key conflicts are handled by rebinding in `keybindings.json`; no extra guard needed
   beyond the hold-alone rule in §8.
2. Platform code for the menu bar item and HUD window in `jisuno-platform` is accepted.
3. Config lives in `~/.config/jisuno/`.
4. Working folder is `~/github/jisuno`.
5. The `voicetools` crate is left as is (no final release, no yank).
6. Reference machine: the owner's MacBook Air M1, 8 GB (§6.1).
7. `kolisachint/jisuno` starts with fresh git history; `voicetools` keeps its own.

**2026-11-26, round 4** (proposals the owner handed to the orchestrator)
1. `jisuno serve` and the hoocode line protocol stay; serve uses the app's pipeline.
2. The whisper backend is dropped. Parakeet only.
3. Model: Parakeet TDT 0.6B **v3** int8 is the default (multilingual). v2 int8 (English) is an
   optional download in Settings → Model and `jisuno models`. One model is loaded at a time.
4. First run writes all four config files in full, every key with its default, each with a
   `"$schema"` key pointing to a local schema in `~/.config/jisuno/schemas/` (written at the
   same time, no network) so editors complete and check them. `keybindings.json` becomes an
   object, `{"$schema", "bindings": [...]}`, so it can carry the key.
5. Insert: paste (with clipboard save/restore) is the default. Type (synthetic key events) is
   the fallback: used for apps listed in `settings.json` `insert_type_apps`, and automatically
   when the clipboard write fails.
6. Text goes to the app that was focused at key-down. If that app quit or its window closed by
   key-up, nothing is inserted anywhere else: the text stays in Recent and the HUD says so
   ("Focus lost: text in Recent").
7. Secure input (a password field or app holding Secure Event Input) blocks the event tap.
   While it is on, the menu bar icon shows a lock badge and the popover names the app. The HUD
   shows "Secure input on: keys paused" once per holding app per session.
8. First hold key wins (§8).
9. A dictation is capped at 2 min (`dictation_limit_s`, default 120; hands-free too). The HUD
   shows a countdown for the last 15 s (from 1:45 with the default); at the cap Jisuno stops and transcribes, as if released.
10. Settings → Audio has a mic picker (default: system input). If the input is Bluetooth, a
    warning explains the quality and latency cost; it also appears once in the HUD. A missing
    device falls back to the system input with a HUD note.
11. A failed step stops the command; the HUD shows which step failed and why (§5.1).
12. Slots use fixed lists where the values are known (`{window}`, `{place}`, `{workspace}`,
    `{theme}`, `{app}` from installed apps). Free text (`{name}` and declared free slots) is
    allowed once per phrase, at the end. Details in `voice-control.md`.
13. Commands take an optional per-app `when` (§5.1).
14. No spoken punctuation and no voice editing ("delete that") in v1; the model's own
    punctuation and casing are used as is.
15. Privacy: the last 5 transcripts are kept in memory only (Recent), gone on quit. No
    telemetry. The only network use is the model download.
16. Distribution: signed and notarized `.app`, as a GitHub release (zip) and a Homebrew cask.
    The CLI is on crates.io as `jisuno` (`cargo install jisuno` builds the CLI only; the app
    is built with the `app` feature for packaging). Updates are manual in v1 (`brew upgrade`
    or a new download); no update check, which keeps rule 15. Revisit Sparkle after v1.

## 11. Open questions

None. Next: owner sign-off of this doc, `keybindings.md`, `voice-control.md`, `app-ui.md` and
`prototypes/v3.html`, then the implementation plan.
