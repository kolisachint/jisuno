# Building

## Prerequisites

| Need | For | Notes |
|---|---|---|
| Rust stable | everything | CI uses `dtolnay/rust-toolchain@stable`. There is no `rust-toolchain` file and no pinned MSRV. |
| macOS 13 or later | the app, `jisuno-platform`, `jisuno-ui` | `bundle.sh` sets `MACOSX_DEPLOYMENT_TARGET=13.0`. |
| Xcode Command Line Tools | the app bundle | `bundle.sh` needs `lipo` and `install_name_tool`; signing needs `codesign`. |
| Rust Apple targets | `bundle.sh --arch universal` (its default) | `rustup target add aarch64-apple-darwin x86_64-apple-darwin`. `--arch arm64` needs only the first. |
| `libasound2-dev` | mic capture on Linux | `sudo apt-get install -y libasound2-dev`. Not needed with `jisuno-audio --no-default-features`. |

The Linux CI job builds the CLI and the pure crates only. Apple silicon CI runs the macOS jobs. See
[Testing](testing.md).

## Workspace and features

`Cargo.toml` is a virtual workspace with `members = ["crates/*"]`. Feature flags:

| Crate | Feature | Default | Effect |
|---|---|:-:|---|
| `jisuno` | `app` | | Menu bar app (GPUI, `jisuno-platform`, `jisuno-ui`, `cpal`). Off: CLI only, which is what `cargo install jisuno` builds. Code is also gated on macOS. |
| `jisuno` | `dev` | | Debug hooks (`--debug-wav`, `--debug-hold-ms`, `--debug-open`, `JISUNO_DEBUG_*`) and, through `jisuno-asr/dev`, the fake helper backend. Never in release builds. |
| `jisuno-audio` | `mic` | yes | `cpal` mic capture. Off: no ALSA. |
| `jisuno-asr` | `dev` | | Fake helper backend (`JISUNO_FAKE_WORKER`) and the `asr_worker` example. |

## Build the CLI

```sh
cargo build --release -p jisuno                 # target/release/jisuno (mic transcription and tools)
cargo run -p jisuno -- transcribe --wav recording.wav
```

`cargo build -p jisuno-audio --no-default-features` builds the audio crate without `cpal` (no ALSA
needed). `jisuno-bench` depends on it that way.

## Build and run the app from source

```sh
cargo build --release -p jisuno --features app
target/release/jisuno app                       # or bare `jisuno`: the app is the default on app builds
```

Run it from a terminal to read the stderr logs. `JISUNO_CONFIG_DIR=/tmp/jisuno-test` points the app at a
separate config folder, so test settings do not touch `~/.config/jisuno`. The model lives in the default
model folder unless `JISUNO_MODELS_DIR` is set.

Permissions (Microphone, Accessibility, Input Monitoring) belong to the binary's signing identity. A
binary built with `cargo` is ad-hoc signed, and macOS asks again after a rebuild. See
[Permissions and signing](#permissions-and-signing).

## Debug hooks (`dev` feature)

Dev builds accept hidden flags on `jisuno app`. `--help` does not list them.

| Flag | Env | What it does |
|---|---|---|
| `--debug-wav <file>` | `JISUNO_DEBUG_WAV` | Feeds the WAV in place of the mic, at real time, while the hold is down. Skips the mic permission. |
| `--debug-hold-ms <ms>` | `JISUNO_DEBUG_HOLD_MS` | Simulates one Right ⌥ hold of this length, starting about 1 s after launch. It bypasses the event tap, so no Input Monitoring is needed. |
| `--debug-open popover\|settings\|welcome` | `JISUNO_DEBUG_OPEN` | Opens one window at launch, for screenshots. |

Example: a 4 s dictation from a WAV file, with no key press:

```sh
cargo build -p jisuno --features app,dev
JISUNO_CONFIG_DIR=/tmp/jisuno-test target/debug/jisuno app \
  --debug-wav recording.wav --debug-hold-ms 4000 2>app.log
```

The fake helper backend (no model; text is the sample count) serves the lifecycle tests. The
`asr_worker` example runs the helper over binary frames on stdin and stdout, so a client drives it.
`JISUNO_FAKE_WORKER=1` selects the fake backend in any `dev` build.

Release builds have none of these. Do not add them to `bundle.sh`.

Developer variables, not in the user guide:

| Variable | Read by | Meaning |
|---|---|---|
| `JISUNO_PROFILE=1` | `jisuno-asr` (`parakeet.rs`) | Per-stage decode timings on stderr. |
| `JISUNO_FAKE_WORKER=1` | `jisuno-asr` worker (`dev` feature) | Fake helper backend. |
| `JISUNO_DEBUG_WAV`, `JISUNO_DEBUG_HOLD_MS`, `JISUNO_DEBUG_OPEN` | `jisuno app` (`dev` feature) | Same as the three debug flags. |
| `JISUNO_FIXTURES` | ignored tests in `jisuno-asr` and `jisuno-bench` | Fixture folder. Default `/tmp/fixtures`. |
| `JISUNO_GALLERY_VIEW`, `JISUNO_GALLERY_PANE`, `JISUNO_GALLERY_DARK` | `windows_gallery` example | Which window, which pane, dark palette. |

## Bundle the app

`packaging/bundle.sh` builds `jisuno` (feature `app`) and `jisuno-bench` for each target, combines them
with `lipo` when `universal`, and assembles `Jisuno.app`:

```sh
packaging/bundle.sh --version 0.1.0 --arch arm64 --out target/packaging
packaging/bundle.sh --version 0.1.0 --arch universal            # default out: target/packaging
```

| Flag | Values | Default |
|---|---|---|
| `--version` | `X.Y.Z`, required | none |
| `--arch` | `arm64`, `x86_64`, `universal` | `universal` |
| `--out` | a directory | `target/packaging` |

Output: `Jisuno.app/Contents/{MacOS/jisuno, MacOS/jisuno-bench, Info.plist, Resources/AppIcon.icns}`, plus
`Frameworks/libonnxruntime*.dylib` if the build produced one. The build uses `--locked`. The bundle is
unsigned.

`jisuno-bench` lives in `MacOS/` next to `jisuno` so that Settings → Performance can run it. A zip for
testing: `ditto -c -k --keepParent target/packaging/Jisuno.app target/packaging/Jisuno.zip`.

`packaging/Info.plist` sets `LSUIElement` (no Dock icon), the microphone usage string and
`LSMinimumSystemVersion` 13.0. `packaging/jisuno.entitlements` grants audio input only, for the
hardened runtime.

## Permissions and signing

- `sign-notarize.sh --app <path> --zip <path>` signs with Developer ID (hardened runtime), notarizes,
  staples and zips. It needs `DEVELOPER_ID_APPLICATION` and one of `NOTARY_KEYCHAIN_PROFILE` or
  `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD`. `--dry-run` prints the commands only.
- TCC grants (Microphone, Accessibility, Input Monitoring) follow the app's designated requirement and
  bundle id. An ad-hoc or differently signed build gets a new identity, so macOS asks again after each
  rebuild. Test permission flows with a build signed by the release identity.
- Login item (`login_item`) registers through `SMAppService.mainApp`. It only runs from a bundle
  (`login_item::in_app_bundle`). A development run logs `login item: needs the bundled app`.

Release signing and CI are in [Release](release.md), with the detailed references in
[`packaging/README.md`](../../../packaging/README.md) and [`ci/README.md`](../../../ci/README.md).

## Linux

The core builds on Linux. This covers the pure crates, the CLI without the app, and the bench crate:

```sh
sudo apt-get install -y libasound2-dev
cargo build --workspace
```

macOS-only code is cross-checked for Linux from a Mac. `cargo clippy` needs the target installed first
(`rustup target add x86_64-unknown-linux-gnu`). Crates that need a C toolchain or ALSA may not build
locally:

```sh
cargo clippy --target x86_64-unknown-linux-gnu -p jisuno-platform -p jisuno-logic -p jisuno-ui --all-targets -- -D warnings
```

## Model files

Most tests and the app need the Parakeet model installed. Fetch it once:

```sh
cargo run --release -p jisuno -- setup                          # v3 int8, default
cargo run --release -p jisuno -- models download parakeet-tdt-0.6b-v2-int8
```

Models are described in the [user guide](../../user/src/models.md). Tests that need a model are marked
`#[ignore]` and skip when it is missing (see [Testing](testing.md)).
