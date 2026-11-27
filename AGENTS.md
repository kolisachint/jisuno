# Development Rules

## Repo Map

`jisuno` is a Cargo virtual workspace (`crates/*`): a local, offline voice-to-text
tool for macOS. It ships a menu bar app (`app` feature) and a `jisuno` CLI. The
app records from a hotkey, runs the model in a helper child process
(`jisuno asr-worker`), inserts text or runs voice commands. The CLI covers
`setup`, `models`, `transcribe`, `serve`, `control`, `model`. Target layout:
`docs/design/jisuno.md` §2. Decisions: `docs/design/jisuno.md` §10 (later rounds win).

Docs (keep these current when behavior changes):

- `README.md` — features, install, usage, CLI, env vars, output protocol, performance, architecture
- `ci/README.md` — CI/release workflow reference
- `docs/user/src/` — mdBook user guide (`SUMMARY.md` lists the pages)
- `docs/user/src/tui.md` + `docs/user/src/tui/voice-transcribe.ts` — TUI push-to-talk wiring reference

Code:

- `crates/jisuno/` — the `jisuno` binary (CLI, and the menu bar app with feature `app`)
  - `src/main.rs` — module declarations and entry point (`cli::run()`)
  - `src/cli.rs` — clap subcommands (`setup`, `models`, `transcribe`, `serve`,
    `control`, `model`, hidden `asr-worker`, `app`), the mic → VAD → resample →
    transcribe pipeline for `transcribe`
  - `src/serve.rs` — persistent daemon: loads models once, then answers
    `START`/`CANCEL`/`SHUTDOWN` on stdin (one capture at a time). Streams live
    `PARTIAL`s by re-decoding the audio-so-far on a `--partial-ms` interval,
    then emits one `FINAL`
  - `src/control.rs` — `jisuno control`: list/add/remove/validate voice commands
    (validates, then writes atomically; exit 2 on validation error)
  - `src/ipc.rs` — one-line JSON protocol on the app's control socket, used by
    `jisuno model status|warm|unload` (client side; server in `app/socket.rs`)
  - `src/runtime.rs` — voice-command runtime values: fixed slot lists, host inventory
  - `src/watch.rs` — config file watcher (poll mtime and size on a thread)
  - `src/app/` — menu bar app (macOS, `app` feature)
    - `mod.rs` — startup (config, permissions, event tap, socket), GPUI run loop, 20 ms tick
    - `state.rs` — `AppState` and its value types
    - `pipeline.rs` — hold → mic → transcribe → insert; tones; Bluetooth/missing-mic notes
    - `worker.rs` — helper client thread: streaming and full-clip decodes
    - `model.rs` — model load, cancel, unload, idle unload, helper respawn
    - `config.rs` — config reload, first-run writes, secure-input poll, tap restart
    - `command.rs` — voice-command execution (runner plans, HUD copy, Recent)
    - `hud.rs` — HUD show, hide, level refresh
    - `status.rs` — menu bar icon state, model line, permission rows
    - `windows.rs` — popover, Settings and Welcome windows
    - `socket.rs` — control socket server (`~/Library/Application Support/jisuno/jisuno.sock`)
- `crates/jisuno-logic/` — pure, no audio/ONNX/network deps
  - `src/hold.rs` — hold-key and utterance state machine (hold-alone, first hold wins, caps)
  - `src/keybindings.rs` — `keybindings.json`: parse, validate, display
  - `src/settings.rs` — `settings.json`: typed settings, parse with line diagnostics, first-run files
  - `src/matcher.rs` — voice-command phrase matcher (`{slot}` templates, fuzzy words)
  - `src/runner.rs` — command steps as data: plans, failure and HUD messages
  - `src/voice_control/` — `voice-control-config.json`: types, line scanner, validation
  - `src/vad.rs` — energy VAD with auto-stop on trailing silence
    (`Speech`/`Silence`/`SilenceStart`/`SilenceTimeout`)
  - `src/protocol.rs` — the stdout line protocol
    (`STATUS`/`SEGMENT`/`DONE`/`ERROR`, plus `READY`/`LEVEL`/`PARTIAL`/`PHASE`/
    `FINAL` for `serve`)
  - `defaults/` and `schemas/` — the four config files' defaults and JSON Schemas (embedded in the binary)
- `crates/jisuno-audio/` — audio helpers, mic capture, cues
  - `src/audio.rs` — mono downmix, anti-aliased resample → 16 kHz (windowed-sinc
    low-pass before decimation), WAV loading, chunked streaming resampler
  - `src/mic.rs` — `cpal` capture (native-rate mono chunks over a channel);
    behind the default `mic` feature
  - `src/tones.rs` — start, stop and cancel cues (synthesized sine, played on a thread)
- `crates/jisuno-asr/` — speech recognition, model files, helper process
  - `src/transcribe/mod.rs` — `Transcriber` and streaming trait + model loading
  - `src/transcribe/parakeet.rs` — ONNX Runtime TDT decode; pause-split streaming (`StreamConfig`)
  - `src/worker/` — helper process: `client.rs` (parent: spawn, framed requests),
    `server.rs` (child: loads the model, answers requests), `frame.rs` (binary framing)
  - `src/setup.rs` — model registry (ids, pinned files, data dir, `JISUNO_MODELS_DIR`), install, remove, list
  - `src/download.rs` — resumable, hash-verified downloads
  - `src/tls.rs` — TLS trust for model downloads
  - `PERF.md` — measured latency, memory and experiments (keep numbers in README in sync)
- `crates/jisuno-platform/` — macOS only (non-macOS builds are stubs): event tap
  (`keys.rs`), text insertion (`insert.rs`), frontmost app, permissions, secure
  input, menu bar status item, HUD placement, and voice-command actions in
  `actions/` (`app`, `ax`, `window`, `workspace`, `shell`, `keys_send`, `wait`)
- `crates/jisuno-ui/` — GPUI views and view models for the app: HUD, popover,
  Settings, Welcome, theme tokens (`tokens.rs`); macOS only, GPUI behind the target cfg
- `crates/jisuno-bench/` — ASR latency, memory and WER harness (binary `jisuno-bench`,
  `publish = false`); `jisuno bench` is not wired yet
- `.github/workflows/` — `ci.yml`, `release.yml`, `mirror-models.yml`
- `.agents/commands/` — slash-command definitions (`pr.md`)

**Cargo features**:

- `jisuno-audio` `mic` (default, `cpal`): mic capture. Off = no ALSA build.
- `jisuno` `app`: the menu bar app (GPUI, `jisuno-platform`, `jisuno-ui`, `cpal`). Off = CLI only,
  which is what `cargo install jisuno` builds. Code is also gated on `target_os = "macos"`.
- `jisuno` `dev`: debug hooks (`--debug-wav`, `--debug-hold-ms`, `--debug-open`, `JISUNO_DEBUG_*`)
  and, through `jisuno-asr/dev`, the fake helper backend (`JISUNO_FAKE_WORKER`). Never in release.
  `bundle.sh` does not enable it.

Parakeet is unconditional. The whisper backend was dropped.

## Conversational Style

- Keep answers short and concise
- No emojis in commits, issues, PR comments, or code
- No fluff or cheerful filler text
- Technical prose only, be kind but direct

## Code Quality

- Read files in full before making wide-ranging changes, before editing files
  you have not already fully inspected, and when asked to investigate or audit.
  Do not rely only on search snippets for broad changes.
- Match the surrounding style: import order, naming, error handling (`anyhow`
  with `?`)
- Avoid `unwrap()`/`expect()` outside tests; thread errors with `?`
- Keep the ONNX input/output tensor names as the `const`s at the top of
  `src/transcribe/parakeet.rs`; the decode logic stays export-agnostic
- Do not preserve backward compatibility unless the user explicitly asks
- Always ask before removing functionality that appears intentional

## Commands

- After code changes (not doc-only changes), run all three and fix everything
  before committing:
  ```bash
  cargo fmt --all --check
  cargo clippy --all-targets -- -D warnings
  cargo test
  ```
- Linux needs ALSA headers for mic capture: `sudo apt-get install -y libasound2-dev`
- Check Linux clippy from macOS (needs `rustup target add x86_64-unknown-linux-gnu`;
  crates that need a C toolchain or ALSA will not build locally):
  `cargo clippy --target x86_64-unknown-linux-gnu -p jisuno-platform -p jisuno-logic -p jisuno-ui --all-targets -- -D warnings`
- Validate a model end-to-end locally with `--wav`:
  `cargo run --release -p jisuno -- transcribe --wav recording.wav`
- If you create or modify a test, run it and iterate until it passes
- NEVER commit unless the user asks

## Slash Commands

- `/pr [patch|minor|major]` — opens a release PR on a feature branch and labels
  it `cargo:<bump>` so `release.yml` bumps the version, publishes to crates.io,
  and builds cross-platform binaries on merge. Defined in `.agents/commands/pr.md`.
  Defaults to `patch`.
- Slash-command definitions live in `.agents/commands/`.

## Releasing

**Version semantics**:

- `patch` — bug fixes and additions
- `minor` — API/behavior changes
- `major` — large breaking changes

### Flow (do NOT bump versions or tag by hand)

**Never edit `version = "…"` in `Cargo.toml` inside a feature PR.** The release
workflow is the sole owner of the version: it takes the base from the highest
`v*` git tag, applies the PR's `cargo:<bump>` label, then rewrites the manifest.
A manual bump is at best ignored and at worst confusing (it can cause a skipped
version number). Leave the version untouched and just apply the label.

1. `/pr <bump>` creates the `cargo:<bump>` label if missing and opens a PR with it.
2. On merge into `main`, `release.yml` picks the version: the highest `v*` tag
   bumped by the label, or the manifest version when no `v*` tag exists yet
   (the first release, `v0.1.0`). It bumps `Cargo.toml` and `Cargo.lock`,
   commits `release: v<version>` and tags it, in one atomic push. A re-run
   reuses the tag and pushes nothing.
3. The same workflow then publishes `jisuno-logic`, `jisuno-audio`, `jisuno-asr`,
   `jisuno` to crates.io in dependency order (skipping a
   version already on the index) and builds + attaches binaries for macOS
   (Apple silicon + Intel), Linux (`x86_64-unknown-linux-gnu`), and Windows to
   the GitHub release.

Secrets required: `CRATES_IO_TOKEN` (crates.io publish). `GITHUB_TOKEN` is
provided automatically; if `main` is protected, set `RELEASE_TOKEN` (see
`ci/README.md`).

Manual fallback (only if asked): `git tag vX.Y.Z && git push origin vX.Y.Z`.

## **CRITICAL** Git Rules for Parallel Agents **CRITICAL**

Multiple agents may work on different files in the same worktree simultaneously.

### Committing

- ONLY commit files YOU changed in THIS session
- Include `fixes #<number>` / `closes #<number>` when there is a related issue/PR
- NEVER use `git add -A` or `git add .` — these sweep up other agents' changes
- ALWAYS `git add <specific-file-paths>` listing only files you modified
- Run `git status` before committing and verify you are staging only YOUR files

### Forbidden Git Operations

These can destroy other agents' work and are never allowed:

- `git reset --hard`
- `git checkout .`
- `git clean -fd`
- `git stash`
- `git add -A` / `git add .`
- `git commit --no-verify`

### Safe Workflow

```bash
git status                       # 1. check first
git add src/vad.rs               # 2. stage only your files
git commit -m "fix(vad): ..."    # 3. commit
git pull --rebase && git push    # 4. push (never reset/checkout)
```

### If Rebase Conflicts Occur

- Resolve conflicts in YOUR files only
- If a conflict is in a file you did not modify, abort and ask the user
- NEVER force push over shared history

### User Override

If the user's instructions conflict with these rules, ask for confirmation that
they want to override. Only then proceed.
