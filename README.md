# jisuno

Local, offline voice-to-text for macOS. Jisuno is a menu bar app and a `jisuno` command-line tool.
Hold a key, speak, let go: the text goes into the app you were using. Hold the other key and say a
command phrase: Jisuno runs the steps you set for it. Speech recognition runs on your Mac with
Parakeet TDT 0.6B (ONNX). **No audio leaves the machine.**

- **Dictate:** hold Right ⌥, speak, release.
- **Command:** hold Right ⌘, say a phrase from `voice-control-config.json`, release.
- **CLI:** `jisuno transcribe` streams text on stdout. `jisuno serve` keeps the model loaded for a
  terminal app (see [Editor / TUI integration](#editor--tui-integration)).

[Install](#install) · [Usage](#usage) · [Commands](#command-line) · [Environment](#environment-variables) ·
[Output protocol](#output-protocol) · [Performance](#performance) · [Privacy](#privacy) ·
[Architecture](#architecture)

Docs: [user guide](docs/user/src/SUMMARY.md) (mdBook) · [design](docs/design/jisuno.md) ·
[CI and release](ci/README.md) · [packaging](packaging/README.md)

## Install

**Menu bar app (macOS, Apple silicon or Intel):**

- Download the Jisuno zip from [Releases](../../releases), unzip it, and move `Jisuno.app` to
  `/Applications`.
- Or: `brew install --cask jisuno`.

**CLI only:**

```bash
cargo install jisuno          # from crates.io; no menu bar app
```

**From source:**

```bash
cargo build --release -p jisuno                      # CLI: target/release/jisuno
cargo build --release -p jisuno --features app       # app build (macOS)
packaging/bundle.sh --arch arm64 --out target/packaging   # Jisuno.app, unsigned
```

`bundle.sh` builds the app itself. Its default `--arch universal` needs both Rust Apple targets.
Signing and notarization: `packaging/sign-notarize.sh`. Linux builds the CLI only and needs ALSA
headers for mic capture: `sudo apt-get install -y libasound2-dev`.

## Usage

Grant Microphone, Accessibility and Input Monitoring when the Welcome window asks. Then:

| Key | What it does |
|---|---|
| Hold **Right ⌥** | Dictate. Release to insert the text into the focused app. |
| Hold **Right ⌘** | Command. Release to run the phrase. |
| **⌃⌥Space** | Start or stop hands-free dictation. Stops after 500 ms of silence. |
| **⌃⌥W** | Load the model ahead of time. |
| **⌃⌥,** | Open Settings. |
| **Esc** | Cancel while listening or confirming. |
| **Return** | Run a command that is waiting for confirmation. |

Holds under 150 ms are taps and do nothing. A hold counts only if no other key is pressed during it.
The first hold key wins. Dictation is capped at 2 minutes. Keys are rebindable in
`~/.config/jisuno/keybindings.json` or in Settings → Keybindings.

The model loads when you first hold a key, or on **⌃⌥W**. It unloads after 10 minutes without use.

Settings has eight panes: General, Audio, Model, Keybindings, Commands, Appearance, Performance and
Files. The menu bar popover shows the model state, the permissions and the last five results (Recent,
in memory only).

### Config files

Four files in `~/.config/jisuno/`, written with every key set to its default on first run:

| File | Holds |
|---|---|
| `settings.json` | Model, mic, limits, insertion, HUD, sounds, login item. |
| `keybindings.json` | Key bindings. |
| `theme.json` | Theme, accent colors, HUD style. |
| `voice-control-config.json` | Voice commands and workspaces. |

Each file has a `$schema` key that points to a local JSON Schema in `~/.config/jisuno/schemas/`. Files
reload when saved. An invalid file is rejected and the last good version stays in use. See the
[config file reference](docs/user/src/config-files.md).

## Models

| Model | Id | Size | Languages |
|---|---|---|---|
| Parakeet TDT 0.6B v3 int8 (default) | `parakeet-tdt-0.6b-v3-int8` | about 650 MB | 25 European languages |
| Parakeet TDT 0.6B v2 int8 (optional) | `parakeet-tdt-0.6b-v2-int8` | about 630 MB | English |

Models are stored in `~/Library/Application Support/jisuno/models/`. Set `JISUNO_MODELS_DIR` to move
them. Downloads come from HuggingFace, with a fallback mirror on the `kolisachint/jisuno` `models-v1`
release. Every file is checked against a pinned sha256 and size, and installed atomically. Weights
are NVIDIA's, under CC-BY-4.0 (see [MODELS_NOTICE.md](MODELS_NOTICE.md)).

## Command line

```text
jisuno                       macOS app build: start the menu bar app. Other builds: same as transcribe.
jisuno setup [--model <id>]  download a model (default v3); --ca-cert <pem>, --insecure for TLS
jisuno models                list models and whether they are installed
jisuno models download <id>  download a model: resumable, hash-verified, atomic install
jisuno models verify <id>    check installed files against their pinned sha256
jisuno models remove <id>    delete an installed model (refused while it downloads)
jisuno transcribe            capture from the mic until silence, then print the text
                             [--model <id>] [--silence-ms <ms>] [--wav <file>]
jisuno serve                 load the model once; answer START/CANCEL/SHUTDOWN on stdin
                             [--model <id>] [--silence-ms <ms>] [--partial-ms <ms>]
jisuno control list          list voice commands (table, or --json)
jisuno control add           add a one-step command; flags per action (see docs)
jisuno control remove <id>   remove a command by id
jisuno control validate      check voice-control-config.json; prints errors and warnings
jisuno model status          print the app's model state: warm, loading or unloaded
jisuno model warm            ask the running app to load the model
jisuno model unload          ask the running app to unload the model
jisuno app                   start the menu bar app (app build only)
```

`jisuno asr-worker` is hidden. The app runs it as the model's helper process.

`control` and `model` are for scripts and agents. `control` edits `voice-control-config.json` and
validates before it writes (exit `2` and nothing written on error). `model` talks to the running app
over `~/Library/Application Support/jisuno/jisuno.sock` and exits `1` with `Jisuno is not running` when
the app is down. See the [agents guide](docs/user/src/agents.md).

Benchmark and WER harness: `cargo run --release -p jisuno-bench -- --help`. Settings → Performance
has **Run benchmark**.

## Environment variables

| Variable | Used by | Meaning |
|---|---|---|
| `JISUNO_CONFIG_DIR` | app, `control` | Config folder. Default `~/.config/jisuno`. |
| `JISUNO_MODELS_DIR` | all | Model folder. Default: the platform data directory (`~/Library/Application Support/jisuno/models` on macOS). |
| `JISUNO_MODEL` | `transcribe`, `serve` | Model id. Default `parakeet-tdt-0.6b-v3-int8`. |
| `JISUNO_SILENCE_MS` | `transcribe`, `serve` | Trailing silence before auto-stop, ms. Default `600`. |
| `JISUNO_PARTIAL_MS` | `serve` | Minimum gap between `PARTIAL` lines, ms. Default `400`; `0` disables partials. |
| `JISUNO_LOG_TEXT` | app | `1` puts dictated and partial text in the logs. Default: counts only. |
| `JISUNO_BIN` | hoocode TUI | Path to the `jisuno` binary the TUI starts. See [Use from a TUI](docs/user/src/tui.md). |
| `SSL_CERT_FILE` | downloads | Extra PEM bundle of trusted CA certificates. |

Flags override their variables. Developer-only variables (`JISUNO_PROFILE`, `JISUNO_FAKE_WORKER`, and the
`JISUNO_DEBUG_*` hooks in `dev` builds) are not listed here.

## Output protocol

Every line on **stdout** is one event. **stderr** carries logs and download progress, so a consumer can
parse stdout cleanly.

`transcribe`:

```text
STATUS recording        # mic capture started (not printed for --wav)
STATUS transcribing     # final decode running
SEGMENT hello           # a chunk of recognized text
SEGMENT world
DONE                    # finished
ERROR <message>         # fatal; process exits non-zero
```

`serve` adds the live lines:

```text
READY                    # models finished loading; ready for START
STATUS listening         # capture started
LEVEL 0.0123             # RMS energy per audio chunk, while listening
PARTIAL hello wor        # interim transcript; replaces the previous PARTIAL
PHASE silence            # trailing silence just started, while listening
STATUS transcribing      # final decode running
FINAL hello world        # committed transcript; DONE follows
DONE                     # capture finished
ERROR <message>          # fatal
```

`PARTIAL` is a best guess and is replaced by the next one: overwrite it, do not append. `FINAL` is the
text to insert. `serve` re-decodes the audio captured so far for each `PARTIAL`.

## Performance

Measured on a MacBook Air M1 with 8 GB RAM, `parakeet-tdt-0.6b-v3-int8`, release build. Numbers come from
[`crates/jisuno-asr/PERF.md`](crates/jisuno-asr/PERF.md).

| Measure | Result | Target |
|---|---|---|
| Key-up → text, warm, 7 s and 17 s clips with pauses | 21–22 ms (median) | ≤ 150 ms |
| Key-up → text, warm, 5.8 s clip with no pause | about 160 ms | ≤ 150 ms (not met) |
| Cold load | about 1.2–1.3 s | ≤ 1.5 s |
| Memory, model warm (helper process) | about 1.1 GB | < 1.2 GB |
| Memory, model unloaded (app) | 32–39 MB | < 60 MB |

Targets are from [`docs/design/jisuno.md`](docs/design/jisuno.md) §6 and §6.1. Dictation is fast because
Jisuno decodes the audio between pauses while you speak, so key-up only decodes the last short segment.
A long burst with no pause is decoded in full at key-up, which is why that clip misses the target. Numbers
depend on the Mac; measure your own with Settings → Performance. Full measurements and method:
[`crates/jisuno-asr/PERF.md`](crates/jisuno-asr/PERF.md).

## Privacy

- Recognition is local. Audio is not sent anywhere.
- The last 5 results are kept in memory only (Recent). They are gone when you quit.
- Logs hold counts, not speech, unless you set `JISUNO_LOG_TEXT=1`.
- No telemetry. No update check.
- The only network use is downloading a model.

## Architecture

```
crates/
├── jisuno/            the binary: CLI and the macOS menu bar app (feature `app`)
│   └── src/
│       ├── main.rs        module declarations, entry point (cli::run)
│       ├── cli.rs         subcommands: setup, models, transcribe, serve, control, model, asr-worker, app
│       ├── serve.rs       daemon: START/CANCEL/SHUTDOWN on stdin, PARTIAL and FINAL on stdout
│       ├── control.rs     `jisuno control`: add/remove/list/validate voice commands
│       ├── ipc.rs         `jisuno model`: one-line JSON over the app's control socket
│       ├── runtime.rs     voice-command runtime values: slot lists and host inventory
│       ├── watch.rs       config file watcher (poll mtime and size)
│       └── app/           menu bar app (macOS, `app` feature)
│           ├── mod.rs       startup, permissions, event tap, run loop
│           ├── state.rs     AppState and its value types
│           ├── pipeline.rs hold → mic → transcribe → insert; tones
│           ├── worker.rs    ASR helper client: streaming and full-clip decodes
│           ├── model.rs     model load, unload, idle unload, helper respawn
│           ├── config.rs    config reload, secure-input poll, tap restart
│           ├── command.rs   voice-command execution
│           ├── hud.rs       HUD show, hide, level
│           ├── status.rs    menu bar icon state, model line, permission rows
│           ├── windows.rs   popover, Settings and Welcome windows
│           └── socket.rs    control socket server
├── jisuno-logic/      pure Rust, no audio, ONNX or network deps
│   └── src/
│       ├── hold.rs          hold-key and utterance state machine
│       ├── keybindings.rs   keybindings.json: parse, validate, display
│       ├── settings.rs      settings.json: types, parse, first-run writes
│       ├── matcher.rs       voice-command phrase matcher
│       ├── runner.rs        command steps as data: plans and failure messages
│       ├── voice_control/   voice-control-config.json: types, scan, validate
│       ├── vad.rs           energy VAD with auto-stop on trailing silence
│       └── protocol.rs      the stdout line protocol
├── jisuno-audio/      audio helpers and mic capture
│   └── src/
│       ├── audio.rs       mono downmix, anti-aliased resample to 16 kHz, WAV loading
│       ├── mic.rs         cpal capture at the device's native rate; feature `mic`
│       └── tones.rs      start, stop and cancel cues
├── jisuno-asr/        speech recognition, model files, helper process
│   └── src/
│       ├── transcribe/      Transcriber trait; parakeet.rs is the ONNX TDT decode
│       ├── worker/          helper process: client (parent), server (child), frame protocol
│       ├── setup.rs         model registry, install, remove, list
│       ├── download.rs      resumable, hash-verified downloads
│       └── tls.rs           TLS trust for downloads
├── jisuno-platform/   macOS glue: event tap, insert, AX actions, windows, permissions
│   └── src/                 keys, insert, frontmost, permissions, secure_input, status_item,
│                            hud_placement, actions/ (app, ax, window, workspace, shell, keys, wait)
├── jisuno-ui/         GPUI views and view models: HUD, popover, Settings, Welcome, theme tokens
└── jisuno-bench/      ASR latency, memory and WER harness (binary jisuno-bench; not published)
```

`jisuno-logic` builds and tests on Linux. The macOS-only crates (`jisuno-platform`, `jisuno-ui`, and the
`app` module) are checked on macOS.

### Helper process

The app does not load the model in its own process. It starts `jisuno asr-worker` when the model loads,
and streams audio to it over a framed stdin/stdout protocol. Unloading stops the helper, so the operating
system takes back all of the model's memory. This is why the app stays small while the model is unloaded.
If the helper crashes, the app marks the model unloaded and starts a new helper on the next hold. The HUD
says `speech helper exited … · restarts on next hold`.

`transcribe` and `serve` load the model in their own process.

### Streaming

While the key is held, the app cuts the audio at pauses (a pause of 0.2 s after at least 1 s of speech)
and decodes each segment in the helper as its own utterance. Key-up decodes only the tail after the last
cut. If 8 s pass without a pause, the audio is cut at the quietest 20 ms frame of the last 2 s. The text of
finished segments is shown as the live partial. Settings → General turns partials off.

`transcribe` and `serve` do not do this. They decode the whole capture at the end (`transcribe`) or
re-decode the audio so far for each `PARTIAL` (`serve`).

### Parakeet decoding

`nemo128.onnx` produces a 128-bin mel spectrogram, `encoder.int8.onnx` encodes it, and the combined
`decoder_joint.int8.onnx` runs greedy Token-and-Duration Transducer (TDT) decoding. The last 5 joint
logits are the duration classes (`NUM_DURATIONS`); the rest are token classes. The vocabulary is read from
`vocab.txt`, so the vocab size is not hard-coded.

The ONNX input and output tensor names are `const`s at the top of
`crates/jisuno-asr/src/transcribe/parakeet.rs`. They match the istupakov and PalatineVision exports. For a
different export, inspect it with [Netron](https://netron.app) and change those constants. The decode logic
itself does not depend on the export. The greedy decode follows
[`jason-ni/parakeet-rs`](https://github.com/jason-ni/parakeet-rs).

ONNX sessions use two intra-op threads with deterministic compute, and the CPU memory arena is off. These
settings were chosen by measurement (see `PERF.md`).

## Cargo features

| Crate | Feature | Default | Effect |
|---|---|:-:|---|
| `jisuno` | `app` | | macOS menu bar app, GPUI and `jisuno-platform`/`jisuno-ui`. Off = CLI only. |
| `jisuno` | `dev` | | Debug hooks: `--debug-wav`, `--debug-hold-ms`, `--debug-open`, the fake helper. Never in release builds. |
| `jisuno-audio` | `mic` | ✓ | `cpal` mic capture. Off = no ALSA build. |
| `jisuno-asr` | `dev` | | Fake helper backend for tests. Enabled by `jisuno`'s `dev`. |

```bash
cargo build --release -p jisuno                                # CLI with mic
cargo build --release -p jisuno --features app                 # menu bar app
cargo test -p jisuno --features app,dev                        # app tests with debug hooks (macOS)
cargo build -p jisuno-audio --no-default-features              # no mic, no ALSA
```

## Releases

CI (`cargo fmt`, `clippy`, `test`) and release automation live in
[`.github/workflows/`](.github/workflows/). See [`ci/README.md`](ci/README.md) for the workflow reference.
Releases are cut with the `/pr <patch|minor|major>` slash command. Merging the labeled PR bumps the version,
tags it, publishes the crates to crates.io, and attaches the macOS app zip, the CLI archives and the
Homebrew cask. Publishing needs the `CRATES_IO_TOKEN` secret.

## Editor / TUI integration

The hoocode TUI can use `jisuno serve` as push-to-talk input on `ctrl+r`. The reference files are
[`docs/user/src/tui.md`](docs/user/src/tui.md) and [`docs/user/src/tui/voice-transcribe.ts`](docs/user/src/tui/voice-transcribe.ts).

## Status

Pre-1.0. The app, the CLI, the voice commands and the release automation are implemented. The
design decisions are in [`docs/design/jisuno.md`](docs/design/jisuno.md) §10. CI does not decode audio on
every push. The `bench` job runs the v3 model on `main` and on manual dispatch, and records numbers without
gating. Validate locally with `--wav`:

```bash
jisuno transcribe --wav recording.wav
```

## Related

A small set of offline-first tools for agents:

- **[hoocode](https://github.com/kolisachint/hoocode)**: deterministic terminal coding agent (TypeScript, on npm)
- **[embeddingsearchtools](https://github.com/kolisachint/embeddingsearchtools)**: semantic and BM25 retrieval, HNSW from scratch (Rust)
- **[webtools](https://github.com/kolisachint/webtools)**: token-efficient web fetch and search (Rust)

Built by [Sachin Koli](https://kolisachint.github.io).

## License

MIT
