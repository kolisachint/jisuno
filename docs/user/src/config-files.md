# Config files

Jisuno keeps its settings in four files in `~/.config/jisuno/`:

| File | What it holds |
|---|---|
| `settings.json` | Model, mic, limits, insertion, HUD, sounds, login item. See the table below. |
| `keybindings.json` | Key bindings. See [Keybindings](keybindings.md). |
| `theme.json` | Theme, accent colors and HUD look. See the `theme.json` section below. |
| `voice-control-config.json` | Voice commands and workspaces. See [Voice commands](commands.md). |

The Settings window edits these files. You can also edit them by hand in any editor.

## First run

On first run Jisuno writes all four files in full, with every key set to its default. It also writes
the JSON Schemas to `~/.config/jisuno/schemas/`. Each file starts with a `$schema` key that points to
its schema in that folder, so editors can complete and check the file. No network is needed.

## Hot reload and errors

- Save a file and the change is live. You do not restart Jisuno.
- If a file is invalid, it is rejected. The last good version stays live.
- The HUD shows the first error once, with `· using last good config`. Settings shows it on the file's
  row.
- `jisuno control validate` prints the same errors.

Errors name the file, the line and the problem:

```
keybindings.json:12: unknown key "hald" · using last good config
```

Keys you leave out of `settings.json` use their defaults. A key Jisuno does not know is an error: the file
is rejected, and the last good version stays live.

`language` was removed in this version. Parakeet v3 detects the language of each utterance, and v2 is English
only. A `settings.json` that still has `language` is rejected. Delete the key.

## settings.json

| Key | Type | Default | Values and meaning |
|---|---|---|---|
| `$schema` | string | none | Path to the local schema. Keep it. |
| `model` | string | `parakeet-tdt-0.6b-v3-int8` | `parakeet-tdt-0.6b-v3-int8` (multilingual) or `parakeet-tdt-0.6b-v2-int8` (English only). See [Models](models.md). |
| `model_load` | enum | `on_demand` | `on_demand` loads when you start speaking. `at_login` loads at login. |
| `unload_after_idle_min` | integer, 0 or more | `10` | Minutes without use before the model unloads. `0` means never. |
| `input_device` | string | `system` | `system` for the system input, or the exact name of a device. If the device is not connected, Jisuno uses the system input and shows a HUD note. Pick it in Settings → Audio. |
| `dictation_limit_s` | integer, 10 to 600 | `120` | Longest dictation in seconds, hands-free included. At the cap Jisuno transcribes as if released. |
| `hands_free_silence_ms` | integer, 200 to 5000 | `500` | Silence in milliseconds that ends a hands-free session. |
| `partials` | boolean | `true` | Show live partial text in the HUD while you speak. Also in Settings → General. |
| `insert` | enum | `paste` | `paste` (with clipboard save and restore) or `type` (key events). |
| `insert_type_apps` | array of bundle IDs | `[]` | Apps that always get typed text instead of paste. |
| `restore_clipboard` | boolean | `true` | Restore the clipboard after a paste. |
| `sounds` | boolean | `true` | Short tones on start, stop and cancel. |
| `hud` | enum | `notch` | Where the HUD appears: `notch`, `cursor` or `off`. |
| `login_item` | boolean | `false` | Launch at login. `true` registers Jisuno as a login item (macOS 13 or later), `false` removes it. Also in Settings → General as **Launch at login**. Registers only from the bundled app (`Jisuno.app`), and the app must be signed. A development run logs a note and changes nothing. |

## keybindings.json

```json
{
  "$schema": "./schemas/keybindings.schema.json",
  "bindings": [
    {"key": "right_option", "hold": true, "command": "voice.dictate"},
    {"key": "escape", "when": "listening", "command": "voice.cancel"}
  ]
}
```

Each entry in `bindings` has:

- `key` (required): the key, spelled as below.
- `command` (required): a command ID. The IDs are listed in [Keybindings](keybindings.md).
- `hold` (optional, default `false`): `true` makes the key a hold key. Only the right-side modifiers
  can be hold keys.
- `when` (optional): `listening` or `confirming`. Leave it out for a global binding.

Key spelling:

- Lowercase. Join parts with `+`.
- Modifiers come first, in this order: `ctrl+option+shift+cmd`. Example: `ctrl+option+w`.
- Modifier names: `ctrl`, `option`, `shift`, `cmd`.
- Hold keys: `right_option`, `right_cmd`, `right_shift`, `right_ctrl`. There are no `left_*` names.
- Key names: letters `a` to `z`, digits `0` to `9`, the punctuation keys `,` `.` `/` `;` `'` `[` `]` `-`
  `=`, and `space`, `escape`, `return`, `tab`, `delete`, `f1` to `f20`.

Uppercase is rejected. Write `ctrl+option+w`, not `Ctrl+Option+W`.

## voice-control-config.json

This file holds the `commands` list and the `workspaces` layouts. It needs `"version": 1` and a `commands`
list. Its format and the voice command rules are described in [Voice commands](commands.md). Agents change it
through `jisuno control`. See [Agents](agents.md).

## theme.json

```json
{
  "$schema": "./schemas/theme.schema.json",
  "name": "quiet-light",
  "appearance": "system",
  "accent_dictate": "#3a85f7",
  "accent_command": "#e39b17",
  "hud": {"style": "island", "show_latency": false}
}
```

| Key | Type | Default | Values and meaning |
|---|---|---|---|
| `$schema` | string | none | Path to the local schema. Keep it. |
| `name` | enum | `quiet-light` | `quiet-light` or `quiet-dark`. The palette, used when `appearance` is `fixed`. |
| `appearance` | enum | `system` | `system` follows macOS light and dark. `fixed` always uses `name`. |
| `accent_dictate` | string, `#rrggbb` | `#3a85f7` | Dictation accent, lowercase hex, for example `#3a85f7`. |
| `accent_command` | string, `#rrggbb` | `#e39b17` | Command accent, lowercase hex, for example `#e39b17`. |
| `hud.style` | enum | `island` | HUD style. `island` is the only value. |
| `hud.show_latency` | boolean | `false` | Show the time in milliseconds after the Inserted line. |

Every key is optional. A key you leave out uses its default. An invalid value is rejected like any other
invalid file: the last good theme stays live. Where the HUD appears is set in `settings.json` (`hud`), and
live partial text is set in `settings.json` (`partials`).

## Environment variables

Flags override their variables. `transcribe` and `serve` also read the model and timing variables.

| Variable | Used by | Meaning |
|---|---|---|
| `JISUNO_CONFIG_DIR` | app, `control` | Config folder. Default `~/.config/jisuno`. |
| `JISUNO_MODELS_DIR` | all | Model folder. Default `~/Library/Application Support/jisuno/models` on macOS. |
| `JISUNO_MODEL` | `transcribe`, `serve` | Model id. Default `parakeet-tdt-0.6b-v3-int8`. |
| `JISUNO_SILENCE_MS` | `transcribe`, `serve` | Trailing silence before the capture ends, in ms. Default `600`. |
| `JISUNO_PARTIAL_MS` | `serve` | Minimum gap between `PARTIAL` lines, in ms. Default `400`. `0` turns partials off. |
| `JISUNO_LOG_TEXT` | app | `1` puts dictated and partial text in the logs. Default: counts only. |
| `SSL_CERT_FILE` | downloads | PEM bundle of extra trusted CA certificates. |
| `JISUNO_BIN` | hoocode TUI | Path to the `jisuno` binary that the TUI starts. See [Use from a TUI](tui.md). |

Developer variables (`JISUNO_PROFILE`, `JISUNO_FAKE_WORKER`, the `JISUNO_DEBUG_*` hooks in `dev` builds, and the
test fixture variables) are in the [developer guide](../../dev/src/building.md).
