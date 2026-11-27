# Agents and the control CLI

`jisuno control` lets a script or an AI agent change voice commands without editing JSON by hand.
It checks the change before writing, so an invalid command is never saved. It writes the file in one
step (a temporary file, then a rename), and Jisuno reloads it.

The file is still plain JSON. You can edit it by hand as well. See [Voice commands](commands.md) for
the format.

## Commands

```
jisuno control list [--json]
jisuno control add --id <id> --say <phrase> --do <action> [<action arguments>] [--confirm] [--when-app <bundle id>]
jisuno control remove <id>
jisuno control validate
```

- `list` prints the commands as a table: ID, phrases and steps. `--json` prints the commands array.
- `add` creates a one-step command. `--say` can be given more than once for several phrases.
  `--confirm` requires a confirmation before the command runs. `--when-app` limits the command to one
  frontmost app, and can be given more than once.
- `remove` deletes a command by ID.
- `validate` checks the file and prints any errors and warnings.

Each argument of the action has its own flag:

| Flag | Used by |
|---|---|
| `--app` | `app.open`, `ax.press`, `ax.set_value` |
| `--arg` | `app.open` (file, URL or extra argument) |
| `--window`, `--place` | `window.focus`, `window.tile` |
| `--workspace` | `workspace.open`, `workspace.close` |
| `--role`, `--name`, `--value` | `ax.press`, `ax.set_value` |
| `--keys` | `keys.send` |
| `--cmd`, `--cwd` | `shell.run` |
| `--theme` | `theme.set` |
| `--ms` | `wait` |

Action names and their arguments are listed in [Voice commands](commands.md#actions).

## Model commands

```
jisuno model status
jisuno model warm
jisuno model unload
```

These talk to the running menu bar app over its control socket,
`~/Library/Application Support/jisuno/jisuno.sock`. The app must be running.

- `status` prints one word: `warm`, `loading` or `unloaded`.
- `warm` starts loading the model and prints `ok`. It returns before the load finishes. Check with
  `status`.
- `unload` unloads the model and prints `ok`.

## Examples

```
$ jisuno control add --id mail --say "open mail" --do app.open --app Mail
added "mail"

$ jisuno control add --id clean --say "clean build" --do shell.run --cmd "cargo clean" --confirm
added "clean"

$ jisuno control list
id          phrases         steps
menu        open jisuno     jisuno.menu
settings    open settings   jisuno.settings
dark        dark mode       theme.set quiet-dark
light       light mode      theme.set quiet-light
warm        warm up         model.warm
mail        open mail       app.open Mail
clean       clean build     shell.run cargo clean

$ jisuno control list --json          # the mail command only, trimmed
[{"id": "mail", "say": ["open mail"], "steps": [{"do": "app.open", "app": "Mail"}], "agent": true}]

$ jisuno control validate
ok

$ jisuno control add --id bad --say "bad thing" --do shell.run --cmd "rm -rf /"
error: command "bad": shell.run requires "confirm": true
(exit 2, nothing written)

$ jisuno control remove mail
removed "mail"

$ jisuno model status
warm
```

`list --json` prints the commands array pretty-printed. The example above is shortened.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success. |
| `1` | Other error: no file, unknown ID, or bad flags. |
| `2` | Validation error. Nothing was written. |

The `jisuno model` commands exit `1` with an error on stderr when they fail:

| Message | Cause |
|---|---|
| `error: Jisuno is not running` | The app is not running, so there is no socket. |
| `error: model not installed` | `warm` ran before the model was downloaded. |
| `error: busy` | `unload` ran while Jisuno was recording or transcribing. Try again when it is idle. |

## Agent-added commands

Commands added by `jisuno control add` are tagged `"agent": true`. Settings lists them separately.
The tag is set on add and is never cleared by the CLI. You can remove it by hand.
