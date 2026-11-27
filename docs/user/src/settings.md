# Settings

Open Settings from the menu bar, or press **⌃⌥,**. Settings has eight panes. Each control writes its file at
once, and the app reloads it. The files are the source of truth. Settings is a view over them: any change
you make in a file shows in Settings, and the reverse.

| Pane | Edits | What it is for |
|---|---|---|
| General | `settings.json` | HUD position, sounds, partials, insert method, clipboard, limits, login item |
| Audio | `settings.json` (`input_device`) | Choose the mic. Test the level meter. |
| Model | `settings.json` (`model`, `model_load`, `unload_after_idle_min`) | Choose, download and load the model |
| Keybindings | `keybindings.json` | Rebind keys. See [Keybindings](keybindings.md). |
| Commands | `voice-control-config.json` (read only here) | List built-in commands, agent-added commands and workspaces |
| Appearance | `theme.json` | Palette, appearance, accents, HUD style and latency |
| Performance | none (reads the benchmark cache) | Target numbers and the benchmark run |
| Files | all four | Open a file in your editor, or copy its last error |

## General

| Control | Key | Default | Notes |
|---|---|---|---|
| Listening cue | `hud` | Notch | Notch, Cursor or Off. Off hides the HUD. Failures still show in the popover and the icon. |
| Sounds | `sounds` | on | Short tones on start, stop and cancel. |
| Live partial text | `partials` | on | Shows finished segments in the HUD while you speak. Needs a warm model. |
| Insert text by | `insert` | Paste | Paste keeps the clipboard. Type is used for apps in `insert_type_apps` and when paste fails. |
| Apps that take typing | `insert_type_apps` | none | Bundle IDs, shown as a list. |
| Restore clipboard after paste | `restore_clipboard` | on | |
| Dictation limit | `dictation_limit_s` | 120 | Seconds. Stepper. The HUD counts down the last 15 s. |
| Hands-free stops after silence | `hands_free_silence_ms` | 500 | Milliseconds. Hold keys ignore this. |
| Launch at login | `login_item` | off | Works only from the installed `Jisuno.app`. See [Install](install.md). |

## Audio

- **Input device.** The system input, or one named device. A device that is not connected falls back to the
  system input with a HUD note. See [Dictation](dictation.md#microphone).
- **Test the microphone.** Shows the level meter for two seconds. Nothing is recorded.
- Warnings appear here for a Bluetooth input (lower quality, more delay).

## Model

- **Model.** Parakeet TDT 0.6B v3 int8 (default, about 650 MB) or v2 int8 (English, optional). One model is
  loaded at a time. Switching unloads the current one.
- **Download.** Fetches the selected model. Progress is shown. It can be resumed.
- **Warm now**, **Unload now.** Load or unload the model. Same as `⌃⌥W` and `jisuno model`.
- **Load.** `On demand` (default) or `At login`. See [Models](models.md).
- **Unload after idle (min).** Default 10. `0` means never.

## Keybindings

Each row is one command with its key and its context. The rows cover every bindable command. Click a key and
press the new chord, or hold the right-side modifier for a hold key. Esc cancels the change.

- If the key is already used, the row names the other command. **Replace** moves the key, and the other
  command loses it. **Cancel** keeps the old key.
- **Set to default** restores one command's key. There is no global reset.
- A common app shortcut shows a warning badge. The binding is accepted.
- An invalid binding is refused with the message from [Keybindings](keybindings.md#reserved-keys).

## Commands

Read only. The pane shows:

- **Built in**: the commands from the file, without `"agent": true`.
- **Added by agents (N)**: commands added with `jisuno control add`. They carry `"agent": true`.
- **Workspaces**: the workspace layouts from the file.

To change a command, edit `voice-control-config.json` or use `jisuno control` ([Agents](agents.md)).
[Voice commands](commands.md) describes the format.

## Appearance

| Control | Key | Values |
|---|---|---|
| Palette | `name` | `quiet-light`, `quiet-dark`. Used when Appearance is `fixed`. |
| Appearance | `appearance` | `system` (follows macOS) or `fixed` |
| Dictate accent | `accent_dictate` | `#rrggbb` |
| Command accent | `accent_command` | `#rrggbb` |
| HUD style | `hud.style` | `island` (the only value) |
| Show latency in HUD | `hud.show_latency` | off by default. Adds the key-up to text time in milliseconds to the `Inserted` line. |

## Performance

The pane shows the targets from the design and the last measured values, with PASS or FAIL for each row.

- **Run benchmark** runs `jisuno-bench` from inside `Jisuno.app`, with the selected model. It uses synthetic
  audio, because the app passes no fixtures. The decode and key-up times are real. Word error rate is `n/a`.
- The result is cached in `~/Library/Caches/jisuno/bench/`. The newest result is shown after a restart.
- If `jisuno-bench` is missing from the bundle, the pane says how to run it from a checkout.

Method and numbers: [Models](models.md#memory-and-speed) and
[`crates/jisuno-asr/PERF.md`](../../../crates/jisuno-asr/PERF.md).

## Files

- **Open file.** Opens one of the four files in your default editor.
- **Copy error.** Copies the last error for that file.
- The folder is `~/.config/jisuno/`. Each file has a `$schema` key, so an editor can check it. See
  [Config files](config-files.md).
