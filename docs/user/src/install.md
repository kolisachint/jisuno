# Install

Jisuno runs on macOS only.

## GitHub release

1. Open the releases page: <https://github.com/kolisachint/jisuno/releases>.
2. Download the Jisuno zip from the latest release.
3. Unzip it and move `Jisuno.app` to `/Applications`.
4. Open Jisuno. An unsigned build is blocked by Gatekeeper. Release builds are signed and notarized. If
   macOS blocks a build, open **System Settings → Privacy & Security** and choose **Open Anyway**.

## Homebrew

```sh
brew install --cask jisuno
```

## Command-line tool only

The `jisuno` CLI is on crates.io. It does not include the menu bar app.

```sh
cargo install jisuno
```

This gives you the `jisuno` command for [Models](models.md), [Agents](agents.md) and
[Use from a TUI](tui.md). You need a Rust toolchain installed. Without the menu bar app, running
`jisuno` with no arguments transcribes from the mic, the same as `jisuno transcribe`.

## Build the app from source

Clone the repository, then build the app with the `app` feature:

```sh
cargo build --release -p jisuno --features app
packaging/bundle.sh --arch arm64 --out target/packaging   # Jisuno.app, unsigned
```

`bundle.sh` runs the same `cargo build --features app` for each target. Its default is `--arch universal`,
which needs both Rust Apple targets. `--arch arm64` builds for Apple silicon only. Signing and notarization are in
`packaging/sign-notarize.sh`. See [`packaging/README.md`](https://github.com/kolisachint/jisuno/blob/main/packaging/README.md).

## Updates

Jisuno does not check for updates. To update, run `brew upgrade` or download the new zip.

## First run

When you first open Jisuno, the Welcome window opens. It:

1. Checks each permission and links to the right System Settings pane. See
   [Permissions](permissions.md).
2. Writes the four config files in full, with every key set to its default, plus a copy of the
   JSON Schemas in `~/.config/jisuno/schemas/`. This needs no network. See
   [Config files](config-files.md).
3. Offers the model download, about 650 MB. See [Models](models.md).
