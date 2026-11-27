# jisuno

Local, offline voice-to-text for macOS. Jisuno is a menu bar app and a `jisuno` command. Hold a key, speak,
release: the text goes into the app you were using. Hold the other key and say a command phrase: Jisuno runs the
steps you set for it. Speech recognition runs on your Mac with Parakeet TDT 0.6B (ONNX). **No audio leaves the
machine.**

- **Dictate:** hold Right ⌥, speak, release.
- **Command:** hold Right ⌘, say a phrase from `voice-control-config.json`, release.

Status: v0.1, pre-release. Builds are unsigned unless a release says otherwise.

## Install

**Menu bar app (macOS 13 or later):**

- Download the Jisuno zip from [Releases](../../releases), unzip it, and move `Jisuno.app` to `/Applications`.
- Or: `brew install --cask jisuno`.

**Command line only:**

```bash
cargo install jisuno        # from crates.io; no menu bar app
```

**From source:**

```bash
cargo build --release -p jisuno --features app                          # the app binary
packaging/bundle.sh --version 0.1.0 --arch arm64 --out target/packaging # Jisuno.app, unsigned
```

`bundle.sh` builds its own binaries. Its default `--arch universal` needs both Rust Apple targets. The CLI also
builds on Linux (`sudo apt-get install -y libasound2-dev` for mic capture).

## Usage

1. Open Jisuno. The Welcome window asks for Microphone, Accessibility and Input Monitoring, and offers the
   model download (about 650 MB).
2. Hold **Right ⌥**, speak, release. The text is inserted into the focused app.
3. Hold **Right ⌘**, say "open settings", release. Jisuno runs the command.

The command line:

```bash
jisuno transcribe                     # speak; prints the text after silence
jisuno transcribe --wav recording.wav # transcribe a file
```

## Documentation

| Where | What |
|---|---|
| [User guide](docs/user/src/SUMMARY.md) | Install, permissions, dictation, voice commands, settings, config files, agents, FAQ |
| [Developer guide](docs/dev/src/SUMMARY.md) | Architecture, building, testing, performance, protocols, release, decisions |
| [Design record](docs/design/README.md) | Signed-off design, decisions in [`jisuno.md`](docs/design/jisuno.md) §10 |
| [PERF.md](crates/jisuno-asr/PERF.md) | Measured latency and memory, with method and rejected experiments |
| [Docs index](docs/README.md) | All documentation in one place |

Agent and release references: [`AGENTS.md`](AGENTS.md), [`ci/README.md`](ci/README.md),
[`packaging/README.md`](packaging/README.md).

## Models

| Model | Id | Size | Languages |
|---|---|---|---|
| Parakeet TDT 0.6B v3 int8 (default) | `parakeet-tdt-0.6b-v3-int8` | about 650 MB | multilingual (25 European languages) |
| Parakeet TDT 0.6B v2 int8 (optional) | `parakeet-tdt-0.6b-v2-int8` | about 630 MB | English |

Models live in `~/Library/Application Support/jisuno/models/`. Downloads are resumable and hash-verified. Weights
are NVIDIA's, under CC-BY-4.0 ([MODELS_NOTICE.md](MODELS_NOTICE.md)). See [Models](docs/user/src/models.md).

## Performance

Measured on a MacBook Air M1 with 8 GB RAM, release build, warm model. Full method in
[`PERF.md`](crates/jisuno-asr/PERF.md).

| Measure | Result | Target |
|---|---|---|
| Key-up → text, speech with pauses (streaming) | 21–22 ms | ≤ 150 ms |
| Key-up → text, 5.8 s with no pause | about 160 ms | ≤ 150 ms (not met) |
| Warm decode, 5 s clip | about 380 ms | ≤ 100 ms (not met) |
| Cold load | about 1.2 s | ≤ 1.5 s |
| Memory, model loaded (helper process) | about 1.1 GB | < 1.2 GB |
| Memory, model unloaded (app) | 32–39 MB | < 60 MB |

The model unloads after 10 minutes without use by default, which returns its memory to the system.

## Privacy

- Recognition is local. Audio is not sent anywhere and is not written to disk.
- No telemetry and no update check. The only network use is downloading a model.
- Recent (the last 5 results) is kept in memory and is gone when you quit.
- Logs hold counts, not speech, unless `JISUNO_LOG_TEXT=1`.

## Status

The app, the CLI, voice commands and the release workflow are implemented. Known gaps between the design and
the code are listed in [Decisions](docs/dev/src/decisions.md#known-differences-between-design-and-code).

## Related

- **[hoocode](https://github.com/kolisachint/hoocode)**: deterministic terminal coding agent (TypeScript, on npm).
  Its push-to-talk uses `jisuno serve`: see [Use from a TUI](docs/user/src/tui.md).
- **[embeddingsearchtools](https://github.com/kolisachint/embeddingsearchtools)**: semantic and BM25 retrieval (Rust).
- **[webtools](https://github.com/kolisachint/webtools)**: token-efficient web fetch and search (Rust).

Built by [Sachin Koli](https://kolisachint.github.io). License: MIT.
