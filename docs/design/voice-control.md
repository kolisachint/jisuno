# Voice control

## 1. Status and purpose

Status: signed off 2026-11-26. Decisions: jisuno.md §10.

This file owns `voice-control-config.json`: the command list (`commands[]`) and the
workspace layouts (`workspaces{}`). Nothing else reads or writes commands.

- Lives at `~/.config/jisuno/voice-control-config.json`.
- Hot-reloaded on save. An invalid file is rejected: the last good config stays live,
  the HUD shows the error once, and `jisuno control validate` repeats it (§7).
- Hand-editable. Agents edit through `jisuno control` (§8), which validates before writing.

## 2. Example

`$schema` points at the local copy written on first run (jisuno.md §10 round 4.4).
Comments are annotations, not part of the file.

```jsonc
{
  "$schema": "./schemas/voice-control-config.schema.json",
  "version": 1,
  "commands": [
    {"id": "menu", "say": ["open jisuno"], "steps": [{"do": "jisuno.menu"}]},
    {"id": "settings", "say": ["open settings"], "steps": [{"do": "jisuno.settings"}]},
    {"id": "work.start", "say": ["start working"], "steps": [{"do": "workspace.open", "workspace": "work"}]},
    {"id": "work.stop", "say": ["stop working"], "steps": [{"do": "workspace.close", "workspace": "work"}]},
    {"id": "focus", "say": ["focus {window}"], "steps": [{"do": "window.focus", "window": "{window}"}]},
    {"id": "move", "say": ["move {window} to {place}"], "steps": [{"do": "window.tile", "window": "{window}", "place": "{place}"}]},
    {"id": "note", "say": ["open note {name}"], "steps": [{"do": "app.open", "app": "Inkuno", "arg": "{name}.md"}]},
    // ^ {name} is the built-in free-text slot: one per phrase, last token (§5).
    {"id": "send", "say": ["send it"], "steps": [{"do": "ax.press", "app": "focused", "role": "AXButton", "name": "Send"}]},
    {"id": "clean", "say": ["clean build"], "confirm": true, "steps": [{"do": "shell.run", "cmd": "cargo clean"}]},
    // ^ shell.run requires "confirm": true, rejected otherwise (§7).
    {"id": "dark", "say": ["dark mode"], "steps": [{"do": "theme.set", "theme": "quiet-dark"}]},
    {"id": "light", "say": ["light mode"], "steps": [{"do": "theme.set", "theme": "quiet-light"}]},
    {"id": "reply", "say": ["reply"], "when": {"app": ["com.apple.mail"]},
     "steps": [{"do": "ax.press", "app": "focused", "role": "AXButton", "name": "Reply"}]},
    // ^ "when": only matches while Mail is frontmost. Filtering happens before matching (§5).
    {"id": "project", "say": ["open project {project}"],
     "slots": {"project": ["jisuno", "inkuno", "rex"]},
     "steps": [{"do": "app.open", "app": "Zed", "arg": "~/github/{project}"}]}
    // ^ declared fixed-list slot: the value list lives in the command (§5).
  ],
  "workspaces": {
    "work": [
      {"id": "terminal", "app": "Rex", "title": "hoocode — ~/github/jisuno", "run": "hoocode", "place": "left-half"},
      {"id": "editor", "app": "Zed", "title": "jisuno — Zed", "open": "~/github/jisuno", "place": "right-top"},
      {"id": "browser", "app": "Safari", "title": "localhost:4000", "open": "http://localhost:4000", "place": "right-bottom"}
    ]
  }
}
```

A workspace window has `id`, `app`, `place`, optional `title`, and at most one of
`open` (path or URL opened in the app) / `run` (shell command started in the window).

## 3. Schema

Draft 2020-12. Ships as `~/.config/jisuno/schemas/voice-control-config.schema.json`.
Structural checks (types, required keys, `place` values, `ms` range, bundle-id shape)
are enforced here; cross-checks (duplicate ids, slot rules, unknown workspaces) are
validation (§7), which runs after schema checks pass.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "./schemas/voice-control-config.schema.json",
  "title": "voice-control-config.json",
  "type": "object",
  "additionalProperties": false,
  "properties": {
    "$schema": { "type": "string" },
    "version": { "const": 1 },
    "commands": { "type": "array", "items": { "$ref": "#/$defs/command" } },
    "workspaces": {
      "type": "object",
      "additionalProperties": { "type": "array", "items": { "$ref": "#/$defs/window" } }
    }
  },
  "required": ["version", "commands"],
  "$defs": {
    "place": {
      "type": "string",
      "enum": ["left-half", "right-half", "top-half", "bottom-half", "left-top", "left-bottom", "right-top", "right-bottom", "full", "center"]
    },
    "slotRef": {
      "type": "string",
      "pattern": "^\\{[a-z_][a-z0-9_]*\\}$"
    },
    "bundleId": {
      "type": "string",
      "pattern": "^[A-Za-z][A-Za-z0-9-]*(\\.[A-Za-z0-9-]+)+$"
    },
    "command": {
      "type": "object",
      "additionalProperties": false,
      "properties": {
        "id": { "type": "string", "minLength": 1 },
        "say": { "type": "array", "minItems": 1, "items": { "type": "string", "minLength": 1 } },
        "steps": { "type": "array", "minItems": 1, "items": { "$ref": "#/$defs/step" } },
        "confirm": { "type": "boolean" },
        "when": {
          "type": "object",
          "additionalProperties": false,
          "properties": {
            "app": { "type": "array", "minItems": 1, "items": { "$ref": "#/$defs/bundleId" } }
          },
          "required": ["app"]
        },
        "slots": {
          "type": "object",
          "additionalProperties": {
            "anyOf": [
              { "type": "array", "minItems": 1, "items": { "type": "string", "minLength": 1 } },
              { "const": "free" }
            ]
          }
        },
        "agent": { "type": "boolean" }
      },
      "required": ["id", "say", "steps"]
    },
    "step": {
      "oneOf": [
        { "$ref": "#/$defs/stepJisunoMenu" },
        { "$ref": "#/$defs/stepJisunoSettings" },
        { "$ref": "#/$defs/stepAppOpen" },
        { "$ref": "#/$defs/stepWindowFocus" },
        { "$ref": "#/$defs/stepWindowTile" },
        { "$ref": "#/$defs/stepWorkspaceOpen" },
        { "$ref": "#/$defs/stepWorkspaceClose" },
        { "$ref": "#/$defs/stepAxPress" },
        { "$ref": "#/$defs/stepAxSetValue" },
        { "$ref": "#/$defs/stepKeysSend" },
        { "$ref": "#/$defs/stepShellRun" },
        { "$ref": "#/$defs/stepThemeSet" },
        { "$ref": "#/$defs/stepModelWarm" },
        { "$ref": "#/$defs/stepModelUnload" },
        { "$ref": "#/$defs/stepWait" }
      ]
    },
    "stepJisunoMenu": {
      "type": "object", "additionalProperties": false,
      "properties": { "do": { "const": "jisuno.menu" } },
      "required": ["do"]
    },
    "stepJisunoSettings": {
      "type": "object", "additionalProperties": false,
      "properties": { "do": { "const": "jisuno.settings" } },
      "required": ["do"]
    },
    "stepAppOpen": {
      "type": "object", "additionalProperties": false,
      "properties": {
        "do": { "const": "app.open" },
        "app": { "type": "string", "minLength": 1 },
        "arg": { "type": "string" }
      },
      "required": ["do", "app"]
    },
    "stepWindowFocus": {
      "type": "object", "additionalProperties": false,
      "properties": {
        "do": { "const": "window.focus" },
        "window": { "type": "string", "minLength": 1 }
      },
      "required": ["do", "window"]
    },
    "stepWindowTile": {
      "type": "object", "additionalProperties": false,
      "properties": {
        "do": { "const": "window.tile" },
        "window": { "type": "string", "minLength": 1 },
        "place": { "anyOf": [ { "$ref": "#/$defs/place" }, { "$ref": "#/$defs/slotRef" } ] }
      },
      "required": ["do", "window", "place"]
    },
    "stepWorkspaceOpen": {
      "type": "object", "additionalProperties": false,
      "properties": {
        "do": { "const": "workspace.open" },
        "workspace": { "type": "string", "minLength": 1 }
      },
      "required": ["do", "workspace"]
    },
    "stepWorkspaceClose": {
      "type": "object", "additionalProperties": false,
      "properties": {
        "do": { "const": "workspace.close" },
        "workspace": { "type": "string", "minLength": 1 }
      },
      "required": ["do", "workspace"]
    },
    "stepAxPress": {
      "type": "object", "additionalProperties": false,
      "properties": {
        "do": { "const": "ax.press" },
        "app": { "type": "string", "minLength": 1 },
        "role": { "type": "string", "minLength": 1 },
        "name": { "type": "string", "minLength": 1 }
      },
      "required": ["do", "app", "role", "name"]
    },
    "stepAxSetValue": {
      "type": "object", "additionalProperties": false,
      "properties": {
        "do": { "const": "ax.set_value" },
        "app": { "type": "string", "minLength": 1 },
        "role": { "type": "string", "minLength": 1 },
        "name": { "type": "string", "minLength": 1 },
        "value": { "type": "string" }
      },
      "required": ["do", "app", "role", "name", "value"]
    },
    "stepKeysSend": {
      "type": "object", "additionalProperties": false,
      "properties": {
        "do": { "const": "keys.send" },
        "keys": { "type": "string", "minLength": 1 }
      },
      "required": ["do", "keys"]
    },
    "stepShellRun": {
      "type": "object", "additionalProperties": false,
      "properties": {
        "do": { "const": "shell.run" },
        "cmd": { "type": "string", "minLength": 1 },
        "cwd": { "type": "string", "minLength": 1 }
      },
      "required": ["do", "cmd"]
    },
    "stepThemeSet": {
      "type": "object", "additionalProperties": false,
      "properties": {
        "do": { "const": "theme.set" },
        "theme": { "type": "string", "minLength": 1 }
      },
      "required": ["do", "theme"]
    },
    "stepModelWarm": {
      "type": "object", "additionalProperties": false,
      "properties": { "do": { "const": "model.warm" } },
      "required": ["do"]
    },
    "stepModelUnload": {
      "type": "object", "additionalProperties": false,
      "properties": { "do": { "const": "model.unload" } },
      "required": ["do"]
    },
    "stepWait": {
      "type": "object", "additionalProperties": false,
      "properties": {
        "do": { "const": "wait" },
        "window": { "type": "string", "minLength": 1 },
        "ms": { "type": "integer", "minimum": 1, "maximum": 10000 }
      },
      "required": ["do"],
      "oneOf": [
        { "required": ["do", "window"] },
        { "required": ["do", "ms"] }
      ]
    },
    "window": {
      "type": "object",
      "additionalProperties": false,
      "properties": {
        "id": { "type": "string", "minLength": 1 },
        "app": { "type": "string", "minLength": 1 },
        "title": { "type": "string" },
        "open": { "type": "string", "minLength": 1 },
        "run": { "type": "string", "minLength": 1 },
        "place": { "$ref": "#/$defs/place" }
      },
      "required": ["id", "app", "place"],
      "not": { "required": ["open", "run"] }
    }
  }
}
```

Notes on the schema:

- `step` is a `oneOf` with one branch per action. Unknown `do` values and unknown or
  missing arguments fail the `oneOf`.
- `stepWait.oneOf` enforces exactly one of `window`/`ms`: both present matches twice,
  neither matches zero times, both fail. `ms` is capped at 10000.
- `window.not` forbids `open` and `run` together. A window with neither is valid: Jisuno focuses the existing window.
- `when.app` items must look like bundle ids; anything else is a schema error.

## 4. Actions

Step syntax: `{"do": "<action>", ...args}`. Values starting with `{` and ending with `}`
are slot references filled at match time (§5); other values are literals.

| Action | Arguments (R required, O optional) | What it does | Failure reasons |
|--|--|--|--|
| `jisuno.menu` | — | Opens the menu bar popover | — |
| `jisuno.settings` | — | Opens the Settings window | — |
| `app.open` | R `app` (installed app name); O `arg` (file, URL, or extra argument) | Launches the app if needed, opens `arg` in it | `app not installed: Foo` |
| `window.focus` | R `window` (workspace window id, or name of a running app) | Focuses the window | `no window "terminal"` |
| `window.tile` | R `window`; R `place` (one of the 10 `place` values, §3) | Moves and resizes the window within the screen's visible frame (menu bar and Dock excluded); `center` is 60% of the visible frame, centered | `no window "terminal"`, `tile failed: no permission` |
| `workspace.open` | R `workspace` (a key of `workspaces`) | Opens each window in turn: launches apps, opens `open` targets, starts `run` commands, places windows | Underlying window reason, e.g. `no window "terminal"`, `app not installed: Foo` |
| `workspace.close` | R `workspace` | Closes the workspace windows | `no window "terminal"` |
| `ax.press` | R `app` (`"focused"` = frontmost app); R `role`; R `name` | Lookup of one element by app + role + name in the focused window: direct attributes first (`AXDefaultButton`, `AXCancelButton`), then a bounded search (depth <= 4, <= 300 elements). Never an unbounded scan | `AX element not found: AXButton "Send"` |
| `ax.set_value` | R `app`; R `role`; R `name`; R `value` | Direct AX lookup, sets the value | `AX element not found: AXButton "Send"` |
| `keys.send` | R `keys` (key spelling as in keybindings) | Sends the key chord to the focused app | `no focused app` |
| `shell.run` | R `cmd`; O `cwd` | Runs the shell command via `/bin/zsh -lc` in `cwd`, stdin closed, 60 s timeout. Command must have `confirm: true` (§7) | `shell exited 1`, `shell timed out after 60 s` |
| `theme.set` | R `theme` (installed theme name) | Switches the theme | `unknown theme "quiet-blue"` |
| `model.warm` | — | Starts loading the model | `model not installed` |
| `model.unload` | — | Unloads the model from memory | — |
| `wait` | Exactly one of: R `window` (waits up to 5 s for the window to exist) / R `ms` (1–10000) | Pauses the step runner | `timed out after 5 s` (window form only) |

## 5. Slots

Built-in fixed-list slots and their value sources:

| Slot | Values |
|--|--|
| `{window}` | Workspace window ids plus names of running apps |
| `{place}` | `left-half`, `right-half`, `top-half`, `bottom-half`, `left-top`, `left-bottom`, `right-top`, `right-bottom`, `full`, `center` |
| `{workspace}` | Keys of `workspaces` |
| `{theme}` | Installed theme names |
| `{app}` | Installed apps |

Built-in free slot: `{name}` (any spoken text).

Declared slots: a command's `slots` object maps a name to a fixed list or to `"free"`:

- Fixed: `"slots": {"project": ["jisuno", "inkuno", "rex"]}` — matches like a built-in list.
- Free: `"slots": {"subject": "free"}` — matches any text.

Rules:

1. At most one free slot per phrase, and it must be the last token. `{name}` counts as
   free. Two free slots, or a free slot anywhere but last, is a validation error (§7).
2. Every `{slot}` in `say` must be a built-in or a declared slot. Every `{slot}` used in
   `steps` must appear in every one of the command's phrases. Both mismatches are errors (§7).
3. A slot value in `steps` is written `"{slot}"` and filled with the matched text.
4. Declared slots cannot use a built-in name (`window`, `place`, `workspace`, `theme`,
   `app`, `name`): check 22.
5. Slot names match `[a-z_][a-z0-9_]*`, in declarations, `say` and `steps`: check 23.
6. Only `{slot}` refs with a slot name are slots. `${...}` is never a slot, so shell text
   such as `echo ${home}` and brace lists such as `mkdir -p x/{src,test}` are left alone.
7. A declaration is `"free"` or a non-empty list of non-empty strings, even when the
   slot is unused: check 25. A fixed value that normalizes to no words is also check 25.
8. A `{slot}` used as a `window.tile` `place` must be `{place}` or a declared list whose
   values are all places: check 24.

Matching:

1. `when` filters first: commands whose `when.app` does not list the frontmost app are
   removed before matching.
2. Normalize both sides with the canonical tokenizer (`matcher::normalize`): lowercase,
   split on whitespace and punctuation (hyphens included), keep `{slot}` tokens, drop
   tokens with no letters or digits. Validation uses the same tokenizer, so `open-settings`
   and `open settings` are the same phrase.
3. Score each phrase by normalized edit distance over words. Fixed-list slot values are
   fuzzy matched the same way as literal words.
4. Per word: a substituted word must be within character distance 0.34 of the phrase word
   and share its first letter. A fixed slot consumes at least one word. A free slot
   captures at most 10 words.
5. Utterances longer than 4x the longest phrase + 8 words are rejected: HUD
   `No command matched`. Nothing runs.
6. The best phrase wins if its distance is at most 20% of the phrase length.
7. Near tie (another candidate within 5%) or nothing accepted: HUD shows
   `Did you mean "start working"?` and nothing runs.
8. Matcher target: under 5 ms.

## 6. Running

1. Steps run in order, each with slots filled from the match.
2. A failed step stops the command. No rollback: earlier steps stay done. HUD:
   `Step 2 of 3 failed: window.tile · no window "terminal"`. Recent shows the failed
   command.
1a. A command whose slots cannot be filled from the match does not start. HUD:
   `Could not run "<label>": <reason>`, for example `Could not run "open project": no matched value for slot "project"`. Nothing runs.
1b. Slot values inside `shell.run` `cmd` are shell-quoted before they are substituted, so
   spoken words cannot add shell syntax. Built-in fixed lists (`{app}`, `{window}`, `{theme}`,
   `{workspace}`, `{place}`) and free text are quoted; only values of a list the command declares
   in its own `slots` are inserted verbatim (round 11).
3. No match: `Did you mean "start working"?`, nothing runs.
4. Confirm flow: a command with `"confirm": true` shows a HUD prompt first:
   `Run "clean build"? cargo clean · Return to run · Esc to cancel`. When the step has a
   `cwd`, the prompt adds it: `Run "clean build"? cargo clean (in ~/github/jisuno) · Return to run · Esc to cancel`.
   Return runs the steps. Esc, or 6 s with no answer, cancels and nothing runs.

## 7. Validation

Runs on hot reload, on `jisuno control validate`, and before every CLI write.
Message format: `voice-control-config.json:<line>: <message>`.
The HUD shows the first error once plus `· using last good config`.

Severity:

- Error: the file is rejected, the last good config stays live, nothing is written.
- Warning: the file is accepted and loaded; the warning appears in `validate` output only.

| # | Check | Message | Example | Severity |
|--|--|--|--|--|
| 1 | File parses as JSON | `invalid JSON: <parser error>` | `voice-control-config.json:1: invalid JSON: unexpected character at line 1 column 1` | error |
| 2 | No unknown keys (file, command, step, `when`, window) | `unknown key "<key>"` | `voice-control-config.json:8: unknown key "confirmm" in command "clean"` | error |
| 3 | Command has `id` | `command <n>: missing "id"` | `voice-control-config.json:5: command 3: missing "id"` | error |
| 4 | Command has `say` | `command "<id>": missing "say"` | `voice-control-config.json:9: command "clean": missing "say"` | error |
| 5 | Command has `steps` | `command "<id>": missing "steps"` | `voice-control-config.json:9: command "clean": missing "steps"` | error |
| 6 | `id` is unique | `duplicate id "<id>"` | `voice-control-config.json:14: duplicate id "clean"` | error |
| 7 | Phrase is unique across commands after normalization (§5); each phrase is compared with every earlier command that has it. Allowed only when the two commands' `when.app` lists do not overlap | `duplicate phrase "<phrase>" (commands "<a>" and "<b>")` | `voice-control-config.json:14: duplicate phrase "open settings" (commands "settings" and "setup")` | error |
| 8 | Phrase is non-empty | `command "<id>": empty phrase in "say"` | `voice-control-config.json:10: command "clean": empty phrase in "say"` | error |
| 9 | `do` is a known action (§4) | `command "<id>": unknown action "<do>"` | `voice-control-config.json:11: command "clean": unknown action "shell.exec"` | error |
| 10 | Required action arguments present | `command "<id>" step <n> (<do>): missing argument "<arg>"` | `voice-control-config.json:11: command "move" step 1 (window.tile): missing argument "place"` | error |
| 11 | No unknown action arguments | `command "<id>" step <n> (<do>): unknown argument "<arg>"` | `voice-control-config.json:11: command "focus" step 1 (window.focus): unknown argument "place"` | error |
| 12 | Slot used in steps appears in every phrase of the command | `command "<id>": slot "{<slot>}" used in steps but not in phrase "<phrase>"` | `voice-control-config.json:12: command "move": slot "{window}" used in steps but not in phrase "move it"` | error |
| 13 | Slot in phrase has a list or declaration | `command "<id>": slot "{<slot>}" in phrase "<phrase>" has no list or declaration` | `voice-control-config.json:12: command "note": slot "{notebook}" in phrase "open note {notebook}" has no list or declaration` | error |
| 14 | At most one free slot per phrase, last token only | `command "<id>": free slot must be the only free slot and the last token: "<phrase>"` | `voice-control-config.json:12: command "note": free slot must be the only free slot and the last token: "open {name} {name}"` | error |
| 15 | `shell.run` needs confirmation | `command "<id>": shell.run requires "confirm": true` | `voice-control-config.json:11: command "clean": shell.run requires "confirm": true` | error |
| 16 | `workspace.open`/`close` names a known workspace | `command "<id>": unknown workspace "<name>"` | `voice-control-config.json:7: command "work.start": unknown workspace "wrk"` | error |
| 17 | Literal `place` is a valid value | `command "<id>": bad place "<place>"` | `voice-control-config.json:12: command "move": bad place "left"` | error |
| 18 | `wait` has exactly one of `window`/`ms` | `command "<id>" step <n> (wait): exactly one of "window" or "ms"` | `voice-control-config.json:13: command "hold" step 1 (wait): exactly one of "window" or "ms"` | error |
| 19 | `wait ms` is in range | `command "<id>" step <n> (wait): "ms" must be <= 10000` | `voice-control-config.json:13: command "hold" step 1 (wait): "ms" must be <= 10000` | error |
| 20 | `when.app` entries are bundle ids | `command "<id>": bad bundle id "<value>"` | `voice-control-config.json:15: command "reply": bad bundle id "mail"` | error |
| 21 | Phrases are not near-duplicates (within the 5% tie threshold of another phrase) | `phrase "<phrase>" is very close to "<other>" (commands "<a>" and "<b>")` | `voice-control-config.json:14: phrase "open setting" is very close to "open settings" (commands "setup" and "settings")` | warning |
| 22 | Declared slot name is not built in (`window`, `place`, `workspace`, `theme`, `app`, `name`) | `command "<id>": slot "<slot>" is built in and cannot be declared` | `voice-control-config.json:9: command "note": slot "app" is built in and cannot be declared` | error |
| 23 | Slot names (declared, in `say` and in `steps`) match `[a-z_][a-z0-9_]*` | `command "<id>": bad slot name "<slot>"` | `voice-control-config.json:9: command "note": bad slot name "Project"` | error |
| 24 | A `{slot}` used as `window.tile` `place` is `{place}` or a declared list of places | `command "<id>": slot "{<slot>}" used as place must be {place} or a list of places` | `voice-control-config.json:12: command "tile": slot "{spot}" used as place must be {place} or a list of places` | error |
| 25 | Declaration is `"free"` or a non-empty list of non-empty strings, even if unused; no fixed value normalizes to zero words | `command "<id>": bad declaration for slot "<slot>"`, or `command "<id>": slot "<slot>" has an empty value` | `voice-control-config.json:9: command "tile": bad declaration for slot "spot"` | error |

Checks 9–11, 17, and 18–20 are also enforced by the schema (§3); validation repeats
them so the messages name the command, the step, and the line.

## 8. CLI for agents

`jisuno control` is the agent path. It validates before writing: exit 2 means a
validation error (§7) and nothing was written. Writes are atomic (temp file + rename)
and the app hot-reloads.

| Command | Flags | What it does |
|--|--|--|
| `jisuno control list` | `--json` | Lists commands: id, phrases, and step summary (table) or the full objects (`--json`) |
| `jisuno control add` | `--id <id>`, `--say <phrase>` (repeatable), `--do <action>`, `--<arg> <value>` per action argument, `--confirm`, `--when-app <bundle id>` (repeatable) | Adds a command, tagged `"agent": true`. `--say` may be given twice for two phrases |
| `jisuno control remove <id>` | — | Removes the command |
| `jisuno control validate` | — | Checks the file, prints errors and warnings (§7) |

Exit codes: 0 ok, 2 validation error (nothing written), 1 other error (no file,
unknown id, bad flags).

Settings shows agent-added commands separately via the `"agent": true` tag. Hand edits
keep or drop the tag freely; the CLI sets it on `add` and never clears it.

Examples:

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

## 9. Out of scope v1

- No LLM in the matcher: fixed phrases plus `{slot}` templates only.
- No unbounded AX traversal: `ax.*` is a direct lookup by app + role + name with a bounded fallback (§4).
- No spoken punctuation: the model's own punctuation and casing are used as is.
- No voice editing ("delete that", "replace X with Y").
- No wake word: commands run from the hold key only.

## Changelog

### Round 7 (owner decisions, 2026-11-26)

1. One tokenizer: `voice_control::normalize` calls `matcher::normalize`. Hyphens split
   words; `open-settings` equals `open settings`; `open note {name}.` is valid.
2. Check 7 compares each phrase with every earlier command that has it, not only the first.
3. New check 22: declared slot names cannot be built-in.
4. New check 23: slot names match `[a-z_][a-z0-9_]*`, including uppercase refs in `say`
   and `steps`.
5. Check 12 runs per phrase and names the phrase.
6. New check 24: a slot used as `window.tile` `place` is `{place}` or a list of places.
7. Slot refs use the runner rule: `${...}` is never a slot.
8. New check 25: malformed declarations are errors even when unused; fixed values that
   normalize to zero words are errors.
9. Bundle ids match the schema pattern: first component starts with a letter, later
   components are `[A-Za-z0-9-]+`.
10. Scanner decodes keys as UTF-8, so non-ASCII keys map to their own line.
11. Matcher: per-word rule (distance 0.34, same first letter), fixed slots consume at least
    one word, free slots capture at most 10 words, long utterances are rejected with
    `No command matched`.
12. Runner: fill failure HUD `Could not run "<label>": <reason>`; free-slot text in
    `shell.run` is shell-quoted; the confirm prompt shows `(in <cwd>)` when `cwd` is set.

### Round 8 (owner decisions, 2026-11-26)

1. `ax.*`: direct attributes first, then a bounded search (depth <= 4, <= 300 elements) in the
   focused window. Never unbounded.
2. `window.tile` `center` = 60% of the visible frame, centered.
3. `shell.run` runs `/bin/zsh -lc` with stdin closed and a 60 s timeout; timeout reason
   `shell timed out after 60 s`.

### Round 11 (pre-release review, 2026-11-27)

1. Runner: `shell.run` `cmd` quotes every slot value except values of lists the command
   declares in its own `slots`. Built-in fixed lists (`{app}`, `{window}`, `{theme}`,
   `{workspace}`, `{place}`) and free text are quoted. Before, built-in fixed lists were verbatim.
