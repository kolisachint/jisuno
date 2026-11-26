# Agents and the control CLI

`jisuno control` lets a script or an AI agent change voice commands without editing JSON by hand.
It checks the change before writing, so an invalid command is never saved. It writes the file in one
step (a temporary file, then a rename), and Jisuno reloads it.

The file is still plain JSON. You can edit it by hand as well. See [Voice commands](commands.md) for
the format.

## Commands

```
jisuno control list [--json]
jisuno control add --id <id> --say <phrase> --do <action> [--<arg> <value>] [--confirm] [--when-app <bundle id>]
jisuno control remove <id>
jisuno control validate
```

- `list` prints the commands as a table: ID, phrases and steps. `--json` prints the full objects.
- `add` creates a command. `--say` can be given twice for two phrases. Each argument of the action
  has its own flag, such as `--app` for `app.open` or `--cmd` for `shell.run`. `--confirm` requires a
  confirmation before the command runs. `--when-app` limits the command to one app, and can be given
  more than once.
- `remove` deletes a command by ID.
- `validate` checks the file and prints any errors and warnings.

Model commands are in the same CLI:

```
jisuno model status|warm|unload
```

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
mail        open mail       app.open Mail

$ jisuno control list --json          # output trimmed to one command
[{"id": "mail", "say": ["open mail"], "steps": [{"do": "app.open", "app": "Mail"}], "agent": true}]

$ jisuno control validate
ok

$ jisuno control add --id bad --say "bad thing" --do shell.run --cmd "rm -rf /"
error: command "bad": shell.run requires "confirm": true
(exit 2, nothing written)

$ jisuno control remove mail
removed "mail"
```

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success. |
| `1` | Other error: no file, unknown ID, or bad flags. |
| `2` | Validation error. Nothing was written. |

## Agent-added commands

Commands added by `jisuno control add` are tagged `"agent": true`. Settings lists them separately.
The tag is set on add and is never cleared by the CLI. You can remove it by hand.
