# Architecture

## Crate map

| Crate | Responsibility | Depends on (internal) | Builds on |
|---|---|---|---|
| `jisuno` | Binary. CLI subcommands, the menu bar app (feature `app`), the hidden `asr-worker` entry, the control socket server. | `jisuno-asr`, `jisuno-audio`, `jisuno-logic`; with `app`: `jisuno-platform`, `jisuno-ui` | macOS (app); any (CLI) |
| `jisuno-logic` | Pure logic: hold state machine, keybindings, settings and voice-command types and validation, matcher, runner, VAD, stdout protocol. | none | all, tested on Linux |
| `jisuno-audio` | Mono downmix, anti-aliased resample to 16 kHz, streaming resampler, WAV load, mic capture (`cpal`, feature `mic`), start/stop/cancel tones. | none | all (`mic` needs ALSA on Linux) |
| `jisuno-asr` | `Transcriber` trait and Parakeet ONNX decode, pause-split streaming, model registry, resumable downloads, TLS trust, helper process (`worker/`). | `jisuno-logic` | all |
| `jisuno-platform` | macOS glue: event tap (`keys`), text insert, frontmost app, permissions, Secure Input, status item, login item, HUD placement, voice-command actions (`actions/`). Stubs on other OSes. | `jisuno-logic` | macOS |
| `jisuno-ui` | GPUI views and view models: HUD, popover, Settings, Welcome, theme tokens. | `jisuno-logic` | macOS (GPUI) |
| `jisuno-bench` | Latency, memory and WER harness. Binary `jisuno-bench`, `publish = false`. Shipped inside `Jisuno.app`. | `jisuno-asr`, `jisuno-audio` (no `mic`) | all |

The file-level layout of each crate is in [`AGENTS.md`](../../../AGENTS.md) (repo map) and in the
`//!` module docs. The design target layout is `docs/design/jisuno.md` §2.

## Dependencies

```
jisuno                         binary: CLI, menu bar app, hidden asr-worker
├── jisuno-logic               pure core; no internal dependencies
├── jisuno-asr ──────────────> jisuno-logic
├── jisuno-audio               no internal dependencies
├── jisuno-platform  [app] ──> jisuno-logic
└── jisuno-ui        [app] ──> jisuno-logic

jisuno-bench ──> jisuno-asr, jisuno-audio (default features off)
```

Rule: anything that can be pure goes into `jisuno-logic` so it is tested on Linux. macOS-only code
stays behind `cfg(target_os = "macos")` and returns an error or a stub elsewhere.

## App runtime

`jisuno::app::run` (`crates/jisuno/src/app/mod.rs`) starts the app. Its threads and processes:

| Thread or process | Started by | Owns | Talks to the tick via |
|---|---|---|---|
| Main thread: GPUI run loop, 20 ms tick | `app::run`; tick in `app/pipeline.rs` (`tick`) | `AppState`, the `HoldMachine`, HUD, popover, Settings and Welcome windows, the status item | channels drained every tick |
| Event tap thread | `KeyTap::start` (`jisuno-platform/src/keys.rs`) | the CGEvent tap and its CFRunLoop | `KeyEvent` over `mpsc`; a `should_capture` callback reads the machine |
| Model worker thread | `worker_main` (`app/worker.rs`), respawned by `app/model.rs` | the `AsrWorker` client, with reader and writer threads (`jisuno-asr/src/worker/client.rs`) | `ToWorker` and `FromWorker` over `mpsc` |
| Helper process `jisuno asr-worker` | `AsrWorker::spawn` (`worker/client.rs`) | the ONNX sessions, so all model memory | framed stdin and stdout (`worker/frame.rs`) |
| Mic callback | `cpal` (`jisuno-audio/src/mic.rs`) | the input stream | native-rate chunks; drained by `drain_mic` |
| Config watcher | `watch::spawn_watcher` (`watch.rs`), 500 ms poll | mtime and size per file | changed paths; re-parsed on the tick |
| Control socket | `socket::start` (`app/socket.rs`): one listener thread, one thread per connection | the Unix socket | a `SocketSnapshot` refreshed on each tick; `warm` and `unload` as `HookRequest` |
| Command thread | `run_command` (`app/command.rs`), one per plan | `MacActions` | `CommandDone` |
| Short-lived threads | model download (`app/model.rs`), bench child (`app/windows.rs`), tones (`jisuno-audio/src/tones.rs`), clipboard restore (`jisuno-platform/src/insert.rs`), mic permission prompt (`app/mod.rs`) | one task each | a channel or nothing |

Rule: the tick never blocks. Model loads, finishes and partials run on the worker thread. Voice-command
steps run on the command thread. `push` to the helper goes through a bounded queue.

### Dictation: key-down to insert

1. **Key-down.** The tap sees Right ⌥ and sends `KeyEvent::HoldDown`. On the next tick, `drain_tap`
   turns it into `Input::HoldDown(HoldKind::Dictate)` for the `HoldMachine`.
2. **Machine effects.** On a warm model the machine returns `OpenMic`. On a cold model it also returns
   `StartModelLoad`. `exec_effects` runs them.
3. **Mic open.** `open_mic` captures the insert target (the frontmost app at key-down), starts the mic,
   and sends `ToWorker::Begin` when the model is warm. If the model is cold, the stream id is reserved
   and starts when the load lands (`flush_cold_stream`).
4. **Streaming.** `drain_mic` resamples each chunk to 16 kHz and sends `ToWorker::Push`. The helper cuts
   the audio at pauses and decodes each segment while the key is held (see [Streaming](#streaming)).
   `poll_partials` asks for a `PARTIAL` every 400 ms while `settings.partials` is on.
   Partial text is the finished segments so far (no re-decode).
5. **Key-up.** The tap sends `HoldUp`. The machine returns `Transcribe { mode, id }` (a release under
   150 ms returns `CloseMic { discard: true }` instead). `begin_transcribe` sends `ToWorker::Finish`
   with the stream id and, on the cold path, the whole clip as fallback.
6. **Decode.** The helper decodes the tail after the last cut and returns `FINAL` (`REP_FINAL`). The
   worker thread forwards it as `FromWorker::Transcript`.
7. **Insert.** `on_transcript` calls `insert_text` (`pipeline.rs`). It adds a leading space when the
   previous insert went to the same app less than 60 s ago and did not end in whitespace. It pastes
   (`jisuno-platform/src/insert.rs`: clipboard saved and restored) or types (`InsertMode::Type`),
   depending on `insert` and `insert_type_apps`.
   If the target app is gone, the text goes to Recent with `not inserted`, and the HUD says
   `Focus lost: text in Recent`.

### Command: key-down to step

Steps 1 to 6 are the same, with `HoldKind::Command` (Right ⌘).

7. **Match.** `resolve_command` (`app/command.rs`) builds the `Runtime` from the last good
   `voice-control-config.json`, the installed apps and the workspace windows. It calls
   `jisuno_logic::matcher` on the transcript and gets a `Resolution`: a match, `No command matched`,
   or `Did you mean "<phrase>"?`.
8. **Plan.** `jisuno_logic::runner` turns the match into a `Plan`: steps with resolved slots, the HUD
   copy and the confirm prompt. A command with `"confirm": true` enters `Confirming` (`show_confirm`)
   and waits for Return, Esc or 6 s.
9. **Run.** `run_command` sends the plan and a workspace snapshot to a command thread. That thread runs
   each step through `MacActions` (`jisuno-platform/src/actions/`), which implements
   `jisuno_logic::runner::Actions`. Hook steps (`model.warm`, `model.unload`, `jisuno.menu`,
   `jisuno.settings`, theme) come back to the tick as `HookRequest`.
10. **Result.** The runner returns an `Outcome`. `CommandDone` updates the HUD and Recent. A failed step
    stops the plan. Steps that ran are not undone.

## Hold state machine

`crates/jisuno-logic/src/hold.rs` is pure. Time is passed in as milliseconds, so the tests are
deterministic. Its rules come from `docs/design/keybindings.md` §4 and §5.

| State | Entered when | Leaves on |
|---|---|---|
| `Idle` | start, after any finish | hold key down |
| `Pending` | hold key down, under 150 ms (mic open, no HUD) | release (tap, discard) or 150 ms (`Listening`) |
| `Listening` | hold passed 150 ms (HUD shown) | release (`Transcribing`), other key (discard), Esc (discard), 2 min cap |
| `HandsFree` | `⌃⌥Space` | `⌃⌥Space` again, 500 ms of silence, Esc, cap |
| `Transcribing` | `Transcribe` handed to the host | `TranscriptReady` or its timeout |
| `Confirming` | a command with `confirm` is resolved | Return (run), Esc or 6 s (cancel) |

Rules that the machine enforces:

- A hold counts only if no other key is pressed during it (`Input::OtherKeyDown`).
- The first hold key wins. The other hold key during a hold is ignored.
- Esc is captured only in `Listening`, `HandsFree` and `Confirming`. Return is captured only in
  `Confirming`. `HoldMachine::captures` decides, and the tap's `should_capture` callback asks it before
  the key is swallowed.
- The dictation cap is `dictation_limit_s` (default 120). The last 15 s are counted down.

## Model lifecycle

```
unloaded ──(hold key, ⌃⌥W, model.warm, "warm up", at_login)──> loading ──> warm
   ^                                                                      │
   └──────── unload (idle timer, model.unload, jisuno model unload) ──────┘
```

- `unloaded` means no helper process. Memory of the app is about 32 to 39 MB (design round 9).
- `loading` starts the helper and sends `LOAD`. A hold that starts a cold load records audio and
  streams it once the load lands.
- `warm` means the helper is running with the ONNX sessions loaded.
- Unload is refused while the mic is open or a transcription is pending (`can_unload`). The socket
  returns `busy` in that case.
- The idle timer (`poll_idle_unload`) uses `unload_after_idle_min` (default 10; 0 = never).
- A helper crash sets the model to unloaded, shows `speech helper exited (…) · restarts on next hold`,
  and the next hold starts a new helper. A failed worker thread is respawned, and the utterance is
  re-driven from its fallback audio.

Why a helper process: unloading exits the child, so the OS returns all model memory (design round 9,
`PERF.md` "Experiment: helper process").

## Streaming

The app streams while the key is held. `jisuno-asr/src/transcribe/parakeet.rs` (`StreamConfig`) cuts the
audio:

- A pause of 0.2 s, after at least 1.0 s of speech since the last cut, cuts the audio. The cut is in the
  middle of the pause.
- After 8 s without a cut, the audio is cut at the lowest-energy 20 ms frame of the last 2 s.
- Energy gate: 20 ms frames, speech above 0.02 RMS, silence below 0.01 RMS.
- Each segment is decoded on a background thread with a fresh predictor state. No state crosses a cut.
- `PARTIAL` returns the joined finished segments. It does not re-decode. Partials therefore appear as
  segments finish, at pauses.
- `FINAL` decodes only the tail after the last cut.

`transcribe` and `serve` do not use the helper. `transcribe` decodes the whole clip at the end. `serve`
re-decodes the audio so far for each `PARTIAL` (see [Protocols](protocols.md)).

## Config files and hot reload

- Four files in `~/.config/jisuno/` (`JISUNO_CONFIG_DIR` overrides). First run writes them with every
  key set to its default, plus the schemas.
- `watch.rs` polls mtime and size every 500 ms. A change is re-parsed on the tick.
- Parse errors keep the last good value. `LastGood` (`jisuno-logic/src/settings.rs`) holds one per file
  (settings, keybindings, voice commands, theme). The HUD shows the first error once, with
  `· using last good config`.
- If the event tap cannot be restarted with a new `keybindings.json`, the old tap stays (commit
  `eeb69af`). The HUD says `Keys paused: Input Monitoring is off · using last good config` when the
  permission is the cause.
- Timing changes (dictation limit) wait until the machine is `Idle` (`session_active`).
- The settings file uses `deny_unknown_fields`: an unknown key rejects the whole file.

## Voice-command matching

`jisuno-logic/src/matcher.rs` is a fixed-phrase matcher with `{slot}` templates. It reads only the
config. There is no model and no accessibility traversal at match time. Constants: a phrase is accepted
at a distance up to 20% of its length; a word substitution counts only when both words start with the
same letter and are within 0.34 by character distance; candidates within 0.05 of each other are a tie
and nothing runs. Filtering by `when` happens before scoring. The rules are in
`docs/design/voice-control.md` §3 to §5 and the `matcher.rs` tests.

## Where to start

| To change | Start at |
|---|---|
| Hold timing or key rules | `jisuno-logic/src/hold.rs`, `docs/design/keybindings.md` |
| Key spelling, reserved keys | `jisuno-logic/src/keybindings.rs` |
| Settings keys | `jisuno-logic/src/settings.rs`, `defaults/settings.json`, `schemas/settings.schema.json` |
| Voice commands | `jisuno-logic/src/voice_control/`, `matcher.rs`, `runner.rs` |
| A voice-command action | `jisuno-platform/src/actions/`, then the `Actions` trait in `runner.rs` |
| Decode, model files | `jisuno-asr/src/transcribe/parakeet.rs`, `setup.rs`, `download.rs` |
| Helper protocol | `jisuno-asr/src/worker/` and [Protocols](protocols.md) |
| HUD text | `jisuno-ui/src/hud_model.rs` (copy), `hud_view.rs` (layout) |
| Control socket | `jisuno/src/app/socket.rs`, `jisuno/src/ipc.rs` |
| CLI flags | `jisuno/src/cli.rs` |
