# Testing

## Gates

Run all three after a code change, before committing. Fix everything they report. Documentation-only
changes skip them.

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs the same checks in more places:

| CI job | Runner | Runs |
|---|---|---|
| `core` (`ci.yml`) | ubuntu-latest | `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace` |
| `macos` (`ci.yml`) | macos-latest (Apple silicon) | clippy on the workspace; clippy with `-p jisuno --features app`; `cargo test --workspace`; `cargo test -p jisuno --features app,dev`; release app build and unsigned bundle |
| `bench` (`ci.yml`) | macos-latest | only on `main` pushes and manual runs; numbers only, never gated (see [Bench](#bench)) |

The `macos` job is the only one that compiles `jisuno-platform` and `jisuno-ui` on a Mac, and the only one
that runs the `dev` tests. Run `cargo test -p jisuno --features app,dev` locally on a Mac before a change
to the app or the platform crates.

Linux clippy cross-check from a Mac: see [Building](building.md#linux).

## Where tests live

- Pure logic (hold machine, matcher, runner, validation, keybindings, settings, protocol, VAD) has unit tests
  in the module, and some integration tests in `crates/jisuno-logic/tests/` (for example `defaults.rs`).
  These run on Linux.
- `crates/jisuno/tests/` runs the built binary (`CARGO_BIN_EXE_jisuno`): `control.rs` for the agent CLI,
  `model_cli.rs` for `jisuno model` without the app (exit 1), and `models.rs` for `jisuno models` against
  a temporary `JISUNO_MODELS_DIR`.
- `crates/jisuno-asr/tests/worker.rs` covers the helper protocol and lifecycle against the fake backend
  (no model). `streaming.rs` covers pause-split streaming.
- `crates/jisuno-bench/tests/determinism.rs` checks that decodes are deterministic.
- macOS-only tests (`jisuno-platform` actions, keys, insert; `jisuno-ui` views) sit behind
  `cfg(target_os = "macos")`. A test that changes the real system (posts keys, opens windows, writes the
  clipboard) is an `example`, not a test. See [Live checks](#live-checks).

## Ignored tests (need a model or fixtures)

Tests that need the Parakeet model or fixture audio are `#[ignore]`. They skip quietly when the model or
the fixtures are missing, so a plain `cargo test` stays green on a clean machine.

| Test | Command | Needs |
|---|---|---|
| Helper parity, cold start and overhead (`worker.rs`, `helper_process_matches_in_process`) | `cargo test --release -p jisuno-asr --test worker -- --ignored --nocapture` | the v3 model |
| Streaming vs full clip (`streaming.rs`) | `cargo test --release -p jisuno-asr --test streaming -- --ignored --nocapture` | the v3 model and fixtures |
| Streaming determinism, edge inputs, quiet utterance (`streaming.rs`) | same command, same needs | the same |
| Threaded decode determinism (`jisuno-bench/tests/determinism.rs`) | `cargo test --release -p jisuno-bench -- --ignored` | the v3 model and fixtures |

Fixtures come from `JISUNO_FIXTURES`, or `/tmp/fixtures` when it is unset. Make them as described in
[`crates/jisuno-bench/fixtures/README.md`](../../../crates/jisuno-bench/fixtures/README.md): a `name.wav`
(any rate, converted to 16 kHz mono) and a `name.txt` with the expected words. Do not commit audio or
transcripts. Install the model first with `jisuno setup` (see [Building](building.md#model-files)).

Run the ignored tests with `--release`. The numbers in [Performance](performance.md) come from release builds.

## Bench

`jisuno-bench` measures what `docs/design/jisuno.md` §6 requires. It is a library (`jisuno_bench`) with a
thin binary.

```sh
cargo run --release -p jisuno-bench -- --fixtures-dir /tmp/fixtures --runs 5 --warmup 1
cargo run --release -p jisuno-bench -- --json --model parakeet-v3 --fixtures-dir /tmp/fixtures
```

| Flag | Default | Meaning |
|---|---|---|
| `--model` | `parakeet-v3` | `parakeet-v3`, `parakeet-v2`, or a registry id. |
| `--fixtures-dir` | none | Directory of `name.wav` and `name.txt` pairs. Without it the base audio is synthetic noise and WER is `n/a`. |
| `--runs` | `5` | Timed samples per measurement. The median is checked. |
| `--warmup` | `1` | Untimed decodes first. |
| `--json` | off | One JSON object on stdout instead of the table. |
| `--gate` | off | Exit 1 when a §6 check fails. Without it the exit code is 0. |
| `--streaming` | off | Also measure the pause-split path: push cost, `finish()` for 5 s and 15 s, streaming WER. Informational. |
| `--stream-push-ms` | `100` | Audio per streaming push. |
| `--stream-config` | `0.2,1.0,8.0` (crate default) | `PAUSE,MIN_SPEECH,MAX_SEG` in seconds. |

What the checks mean:

| Check | Target | Note |
|---|---|---|
| warm decode, 5 s | ≤ 100 ms median | full-clip decode of a 5 s clip |
| key-up → text (5 s) | ≤ 150 ms median | full-clip decode at key-up, the no-pause worst case. The app's pause-split path is measured with `--streaming` and in [Performance](performance.md). |
| cold load, 2 s and 15 s decodes, flush, insert | shown, not gated | Their rows show a target but do not affect `--gate`. |

Memory is reported (RSS warm and after unload) but not checked. Streaming checks are informational and never
affect `--gate`.

Fixture rules: the first sorted `*.wav` is the base audio for the 2, 5 and 15 s clips (sliced or looped). Every
`*.wav` with a sibling `*.txt` is scored for WER. Without any pair, WER is `n/a`.

### Bench in the app

Settings → Performance runs `jisuno-bench --json --model <id>` from the app bundle, with no fixtures
directory. The base audio is therefore synthetic noise, so the decode times are real and the WER is `n/a`.
The JSON is cached in `~/Library/Caches/jisuno/bench/`, and the newest file is shown.

### CI bench

`ci.yml` `bench` runs on `main` and on manual dispatch. It caches the v3 model, generates two `say`
fixtures with `afconvert` to 16 kHz mono, and uploads `jisuno-bench` JSON as an artifact. It never gates.
Shared runners are not the reference machine. Sign-off numbers come from the reference Mac.

## Live checks

These need a Mac with the permissions granted. Use an isolated config folder (`JISUNO_CONFIG_DIR`) so the
user's files are not changed.

| Check | Command | Notes |
|---|---|---|
| Hold without a key press | `cargo run -p jisuno --features app,dev` with `--debug-wav` and `--debug-hold-ms` (see [Building](building.md#debug-hooks-dev-feature)) | Feeds a WAV through the real pipeline and the real model. Needs no Input Monitoring. |
| Synthetic Right ⌥ or Right ⌘ hold at HID level | `cargo run -p jisuno-platform --example hold_sim -- 800 option` (or `command`) | Posts a modifier down and up. The hold must exceed 150 ms. The app must be running with the tap. |
| Live tap and machine decisions | `cargo run -p jisuno-platform --example tap_probe` | Needs Input Monitoring. Prints every key event and the machine's answer. |
| Insert into the frontmost app | `cargo run -p jisuno-platform --example insert_probe -- "hello" paste` | Waits 3 s, so click the target first. `type` tests typed insert. |
| Safe voice actions | `cargo run -p jisuno-platform --example action_probe` | Opens TextEdit, tiles it and restores it. No `shell.run` or `keys.send`. |
| Model state and warm | `jisuno model status`, `jisuno model warm`, `jisuno model unload` | Needs the running app. |
| Screenshots of windows | `JISUNO_GALLERY_VIEW=settings cargo run -p jisuno-ui --example windows_gallery` (`JISUNO_GALLERY_PANE`, `JISUNO_GALLERY_DARK`) and `cargo run -p jisuno-ui --example hud_gallery` | GPUI galleries; no model needed. |
| Screen capture | `screencapture -x /tmp/shot.png` | A macOS command. Not wired into scripts. |

Test WAVs without a microphone:

```sh
say -o /tmp/t.aiff "the quick brown fox jumps over the lazy dog"
afconvert -f WAVE -d LEI16@16000 -c 1 /tmp/t.aiff /tmp/t.wav
jisuno transcribe --wav /tmp/t.wav
```

The helper can be driven by hand with `asr_worker` (dev feature), which speaks the binary protocol in
[Protocols](protocols.md#helper-process-binary-framing).

Logs from a live run go to stderr. `JISUNO_LOG_TEXT=1` adds the dictated text. Use it only for debugging.

## Known flaky areas

- The model download tests use a local test server (`jisuno-asr`). The flaky server was fixed in `bcc748b`, and
  its sockets were made non-blocking in `3158a41`. If a download test hangs, check the test server first.
- Timing on a loaded machine. Latency numbers in [Performance](performance.md) were measured with other work
  running, so the bench reports medians. Do not gate a unit test on wall-clock time.
- Live tests (`examples/`) depend on the Mac: permissions, focus, the audio device. They are not in `cargo test`.
