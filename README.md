# jisuno

Local, offline **voice-to-text for the terminal**. A small Rust binary that
captures your microphone, stops automatically when you go quiet, and streams
recognized text on stdout using a simple line protocol — designed to drop into
a TUI (e.g. hoocode) as a push-to-talk input. **No audio ever leaves the
machine.**

```bash
jisuno setup --model parakeet-tdt-0.6b-v3-int8   # once — ~650 MB, 25 languages
jisuno transcribe                  # speak; stops after ~600ms of silence
```

- **Parakeet-TDT** (NVIDIA) via ONNX Runtime — fast, multilingual, int8.
- Models are pre-exported and downloaded on first run — no Python/NeMo needed.
- ~Single static binary; models live in your user data dir.

[Install](#install) · [Output protocol](#output-protocol) ·
[Architecture](#architecture) · [Status](#status)

## Install

Grab a binary from [Releases](../../releases), or build from source:

```bash
# Linux needs ALSA headers for mic capture
sudo apt-get install -y libasound2-dev   # Debian/Ubuntu

cargo build --release -p jisuno
# binary at target/release/jisuno
```

## First run: download a model

```bash
jisuno setup --model parakeet-tdt-0.6b-v3-int8     # recommended, ~650MB, 25 languages
# or
jisuno setup --model parakeet-tdt-0.6b-v2-int8     # English-only, ~631MB

jisuno models                        # list what's installed
```

Models are stored under your platform data directory, e.g.
`~/.local/share/jisuno/models/` on Linux.

## Transcribe

```bash
# From the mic — records until ~600ms of silence, then transcribes:
jisuno transcribe

# Tune the trailing-silence auto-stop:
jisuno transcribe --silence-ms 800

# From a WAV file (any rate/channels; great for validating a model):
jisuno transcribe --wav recording.wav
```

Environment variables: `JISUNO_MODEL`, `JISUNO_SILENCE_MS`.

## Serve (persistent daemon)

`transcribe` reloads the ONNX models on every invocation and only decodes
*after* you stop talking. `serve` loads the models once and then answers
commands on stdin, so repeated captures are instant — and it transcribes
**live**, streaming interim text as you speak rather than dumping it all at
the end:

```bash
jisuno serve
# stdin:  START            begin mic capture + VAD
#         CANCEL           stop the current capture without transcribing
#         SHUTDOWN         exit gracefully
```

While listening it re-decodes the audio-so-far on a short interval and emits a
`PARTIAL` line; at the end it emits one authoritative `FINAL`. Flags:
`--model`, `--silence-ms`, and `--partial-ms` (min gap between partials,
default 400; `0` disables partials). Env vars `JISUNO_MODEL`,
`JISUNO_SILENCE_MS`, `JISUNO_PARTIAL_MS`. One capture runs at a time; the mic
opens on `START` and closes when it stops.

## Output protocol

Every line on **stdout** is one event; **stderr** carries human logs and
download progress, so a consumer can parse stdout cleanly:

```text
STATUS recording        # capture started (mic only; no capture for --wav)
STATUS transcribing     # final decode running
SEGMENT hello           # a chunk of recognized text (usually one word)
SEGMENT world
DONE                    # finished
ERROR <message>         # fatal; process exits non-zero
```

`serve` speaks the same protocol plus daemon-only lines for the live UI:

```text
READY                    # models finished loading; ready for START
STATUS listening         # capture started
LEVEL 0.0123             # live RMS energy per audio chunk, while listening
PARTIAL hello wor        # interim transcript so far; replaces the last PARTIAL
PHASE silence            # trailing silence just started, while listening
STATUS transcribing      # final decode running
FINAL hello world        # committed transcript; DONE follows
DONE                     # capture finished
ERROR <message>          # fatal
```

`PARTIAL` streams the best guess while you speak and is superseded by the next
one (overwrite, don't append); `FINAL` is the text to insert. `serve` uses
`PARTIAL`/`FINAL` in place of the batch `SEGMENT` that `transcribe` streams.

## Architecture

```
crates/
├── jisuno/            the binary
│   └── src/
│       ├── main.rs        module declarations, entry point
│       ├── cli.rs         CLI + subcommands, mic→VAD→transcribe pipeline
│       └── serve.rs       persistent daemon: streaming START/CANCEL/SHUTDOWN over stdin
├── jisuno-logic/      pure, no audio/ONNX/network deps
│   └── src/
│       ├── vad.rs         energy VAD with auto-stop on trailing silence
│       └── protocol.rs    the stdout line protocol
├── jisuno-audio/      audio helpers and mic capture
│   └── src/
│       ├── audio.rs       mono downmix, anti-aliased resample → 16 kHz, WAV loading
│       └── mic.rs         cpal capture (native-rate mono chunks); feature `mic`
└── jisuno-asr/        speech recognition and model files
    └── src/
        ├── setup.rs       model registry + HuggingFace download wizard
        ├── tls.rs         TLS trust for model downloads
        └── transcribe/
            ├── mod.rs         Transcriber trait + model loading
            └── parakeet.rs    ONNX Runtime: nemo128 → encoder → decoder_joint (TDT)
```

Audio is captured at the device's native rate, VAD runs on those chunks (RMS
is rate-independent), and the buffer is resampled to 16 kHz before inference.
Resampling is anti-aliased — a windowed-sinc low-pass runs before decimation
so high frequencies aren't folded back into the speech band (which garbles
recognition). `transcribe` resamples and decodes the whole utterance once at
the end; `serve` re-runs that pipeline on the audio-so-far at intervals to
stream live `PARTIAL`s, then decodes the full buffer once more for `FINAL`.

### Parakeet decoding

`nemo128.onnx` produces a 128-bin mel spectrogram, `encoder.int8.onnx` encodes
it, and the combined `decoder_joint.int8.onnx` runs greedy Token-and-Duration
Transducer (TDT) decoding. The token/duration logit split is derived from the
joint output width at runtime (`width − NUM_DURATIONS`), so the same code works
for v2 and v3 without hardcoded vocab sizes.

The ONNX input/output tensor names are `const`s at the top of
`crates/jisuno-asr/src/transcribe/parakeet.rs`. They match the istupakov/PalatineVision export;
if you use a differently-named export, inspect it with
[Netron](https://netron.app) and adjust those constants — the decode logic
itself is export-agnostic. Decode flow is adapted from
[`jason-ni/parakeet-rs`](https://github.com/jason-ni/parakeet-rs).

## Status

**v0.1.** The mic → VAD → resample pipeline, the setup/download wizard, the CLI
and the stdout protocol are implemented and unit-tested. The Parakeet TDT decode
has been validated end-to-end against the real `parakeet-tdt-0.6b-v3` int8
weights — a clean transcription of a reference speech clip — but CI cannot run
it, because the models are large and gated, so validate locally with `--wav`.

ONNX sessions run **single-threaded on purpose**: multi-threaded int8 inference
is non-deterministic and makes the greedy decode drop or garble words (see
`build_session` in `crates/jisuno-asr/src/transcribe/parakeet.rs`).

## Cargo features

| Crate          | Feature | Default | Effect                                  |
|----------------|---------|:-------:|-----------------------------------------|
| `jisuno-audio` | `mic`   |   ✓     | `cpal` mic capture; off = no ALSA build |

Parakeet (ONNX) is always built; the whisper backend was dropped.

```bash
cargo build --release -p jisuno                              # with mic
cargo build -p jisuno-audio --no-default-features            # no mic, no ALSA
```

## Releases

CI (`cargo fmt`/`clippy`/`test`) and release automation live in
[`.github/workflows/`](.github/workflows/) (`ci.yml`, `release.yml`); see
[`ci/README.md`](ci/README.md) for the workflow reference. Releases are cut via
the `/pr <patch|minor|major>` slash command: merging the labeled PR bumps the
version, tags, publishes to crates.io, and attaches macOS/Linux/Windows
binaries. Requires the `CRATES_IO_TOKEN` secret.

## Editor / TUI integration

Reference files for wiring `jisuno` into the hoocode TUI as a `ctrl+r`
push-to-talk input live in [`docs/user/src/tui/`](docs/user/src/tui/):
[`tui.md`](docs/user/src/tui.md) and `voice-transcribe.ts`.

## Related

A small set of offline-first tools for agents:

- **[hoocode](https://github.com/kolisachint/hoocode)** — deterministic terminal
  coding agent (TypeScript, on npm)
- **[embeddingsearchtools](https://github.com/kolisachint/embeddingsearchtools)** —
  semantic + BM25 retrieval, HNSW from scratch (Rust)
- **[webtools](https://github.com/kolisachint/webtools)** — token-efficient web
  fetch and search (Rust)

Built by [Sachin Koli](https://kolisachint.github.io).

## License

MIT
