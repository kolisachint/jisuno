# Install

Jisuno runs on macOS only.

## GitHub release

1. Open the releases page: <https://github.com/kolisachint/jisuno/releases>.
2. Download the Jisuno zip from the latest release.
3. Unzip it and move `Jisuno.app` to `/Applications`.
4. Open Jisuno. The app is signed and notarized.

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
[Use from a TUI](tui.md). You need a Rust toolchain installed.

## Updates

Jisuno does not check for updates. To update, run `brew upgrade` or download the new zip.

## First run

When you first open Jisuno it:

1. Checks each permission and links to the right System Settings pane. See
   [Permissions](permissions.md).
2. Writes the four config files in full, with every key set to its default, plus a copy of the
   JSON Schemas in `~/.config/jisuno/schemas/`. This needs no network. See
   [Config files](config-files.md).
3. Offers the model download, about 650 MB. See [Models](models.md).
