# Voice commands

Hold **Right Command**, say a phrase, and release. Jisuno matches the phrase to a command in
`voice-control-config.json` and runs that command's steps in order.

Commands are fixed phrases with optional `{slot}` values. Jisuno does not use an AI model to read
your intent, and it does not scan other apps' interfaces when you speak. The matcher reads only
the config file, so matching is fast.

## Examples

These examples come from the sample config in the design. A new install has only the default
commands: "open jisuno", "open settings", "dark mode", "light mode" and "warm up".

| Say | What runs |
|---|---|
| "open jisuno" | Opens the menu bar popover. |
| "open settings" | Opens Settings. |
| "start working" | Opens the `work` workspace: launches the windows listed for it and places them. |
| "stop working" | Closes the `work` workspace windows. |
| "focus terminal" | Focuses the window `terminal`, or a running app named "terminal". |
| "move terminal to right half" | Moves and resizes the window `terminal` to the right half of the screen. |
| "open note standup" | Opens `standup.md` in Inkuno. `standup` is the free-text slot `{name}`. |
| "send it" | Presses the button named Send in the focused app. |
| "clean build" | Asks you to confirm, then runs `cargo clean`. |
| "dark mode" / "light mode" | Switches the theme. |
| "reply" | Presses the button named Reply. Runs only when Mail is the frontmost app. |
| "open project jisuno" | Opens `~/github/jisuno` in Zed. `jisuno` is a fixed-list slot. |

## Slots

A phrase can contain `{slot}` values. Jisuno fills each slot from the words you say.

- **Fixed lists** (`{window}`, `{place}`, `{workspace}`, `{theme}`, `{app}`). The values come from
  your workspace windows, running apps, the 10 window places, workspace names, themes and
  installed apps.
- **Declared lists.** A command can declare its own list, as `project` does in the example above.
- **Free text** (`{name}`, or a declared slot set to `"free"`). Any words are accepted. A phrase
  can have at most one free slot, and it must be the last word.

Window places: `left-half`, `right-half`, `top-half`, `bottom-half`, `left-top`, `left-bottom`,
`right-top`, `right-bottom`, `full`, `center`. Spoken, they read as "left half", "right top" and so
on.

## How a phrase is matched

1. Commands that have a `when` list are checked against the frontmost app. Commands that do not
   match it are dropped.
2. Jisuno compares the words you said with each phrase. Small differences are allowed, such as a
   misheard word.
3. The best phrase runs if it is close enough. If two phrases are almost equally close, nothing
   runs.
4. If nothing matches, the HUD says `Did you mean "start working"?` (with the closest phrase) and
   nothing runs.

## Confirmation

A command with `"confirm": true` does not run at once. The HUD shows the command and waits:

```
Run "clean build"? cargo clean · Return to run · Esc to cancel
```

Press **Return** to run it. Press **Esc**, or wait 6 seconds, to cancel. Any command with a
`shell.run` step must have `"confirm": true`. Jisuno rejects the file otherwise.

## Per-app commands

A command can run only in certain apps:

```json
{"id": "reply", "say": ["reply"], "when": {"app": ["com.apple.mail"]},
 "steps": [{"do": "ax.press", "app": "focused", "role": "AXButton", "name": "Reply"}]}
```

`when.app` takes bundle IDs. Mail's ID is `com.apple.mail`.

## When a step fails

The command stops at the failed step. Steps that already ran are not undone. The HUD names the
step and the reason, for example:

```
Step 2 of 3 failed: window.tile · no window "terminal"
```

Recent shows the failed command. Common reasons:

| Message | Cause |
|---|---|
| `app not installed: Foo` | The app named in `app.open` or a workspace is not installed. |
| `no window "terminal"` | No window with that ID is open. |
| `tile failed: no permission` | Accessibility permission is missing. See [Permissions](permissions.md). |
| `AX element not found: AXButton "Send"` | The focused app has no button with that role and name. |
| `no focused app` | There is no frontmost app for `keys.send`. |
| `shell exited 1` | The command ran and exited with a non-zero status. |
| `unknown theme "quiet-blue"` | No installed theme has that name. |
| `model not installed` | `model.warm` ran before a model was downloaded. |
| `timed out after 5 s` | A `wait` on a window did not see the window in time. |

## Actions

Each step has `"do"` and its arguments.

| Action | Arguments | What it does |
|---|---|---|
| `jisuno.menu` | none | Opens the menu bar popover. |
| `jisuno.settings` | none | Opens Settings. |
| `app.open` | `app`; optional `arg` | Opens the app, and `arg` (a file, URL or extra argument) in it. |
| `window.focus` | `window` | Focuses a workspace window or a running app. |
| `window.tile` | `window`, `place` | Moves and resizes a window. |
| `workspace.open` | `workspace` | Opens each window of a workspace: launches apps, opens targets, runs commands, places windows. |
| `workspace.close` | `workspace` | Closes a workspace's windows. |
| `ax.press` | `app`, `role`, `name` | Presses one button found by app, role and name. `"focused"` means the frontmost app. |
| `ax.set_value` | `app`, `role`, `name`, `value` | Sets the value of one field found the same way. |
| `keys.send` | `keys` | Sends a key combination to the focused app, written as in [Keybindings](keybindings.md). |
| `shell.run` | `cmd`; optional `cwd` | Runs a shell command. Needs `"confirm": true`. |
| `theme.set` | `theme` | Switches the theme. |
| `model.warm` | none | Loads the model. |
| `model.unload` | none | Unloads the model. |
| `wait` | `window` or `ms` (1 to 10000) | Pauses for a window to appear (up to 5 s) or for a time. |

## Editing commands

Edit `voice-control-config.json` by hand, or let an agent or script change it with
`jisuno control` (see [Agents](agents.md)). Jisuno reloads the file when it changes. See
[Config files](config-files.md) for how invalid files are handled.

Errors name the file, the line and the problem, for example:

```
voice-control-config.json:11: command "clean": shell.run requires "confirm": true
```

The file is rejected and the last good version stays live.
