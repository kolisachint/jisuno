# Keybindings — source, contract, changelog

## 1. Status and source of truth

Status: signed off 2026-11-26. Decisions: `jisuno.md` §10.

- Source of truth (planned): `defaults()` in `jisuno-logic`. No code exists yet; when code lands, `defaults()` wins over this doc.
- File: `~/.config/jisuno/keybindings.json`, an object:
  ```json
  {"$schema": "./schemas/keybindings.schema.json", "bindings": [{"key": "right_option", "hold": true, "command": "voice.dictate"}]}
  ```
  Each binding is `{key, hold?, when?, command}`. `hold` and `when` are optional; absent `when` means global (§5).
- First run writes the file in full (every default binding, plus `$schema`). Schemas are written at the same time under `~/.config/jisuno/schemas/`; no network.
- Hot reload: edits are picked up live. On error the file is rejected and the last good config stays live; the error is shown once in the HUD and in Settings (see §8).

## 2. Principles (one line each + its cost)

1. **Hotkey only (no wake word).** The mic opens on key-down only. Cost: no hands-free-by-voice; hands-free starts from its chord (§4).
2. **Two hold keys, not one key that guesses the mode.** Right ⌥ dictates, Right ⌘ commands. Cost: two keys to learn instead of one.
3. **Right-side modifiers alone because they are free on default macOS.** Cost: both hold keys sit on the right of the keyboard; rebind if that hurts.
4. **Never steal a chord (hold-alone rule).** A hold counts only if no other key is pressed during it. Cost: a sloppy hold that brushes another key does nothing.
5. **Everything rebindable.** Every command in §10 can be rebound, unbound, or re-set to default in Settings → Keybindings. Cost: validation and conflict UI must exist.
6. **Conflicts solved by rebinding, not by heuristics.** No priority rules, no guessing which action the user meant. Cost: the user fixes duplicates by hand (§8, §9).

## 3. Default chord table

Matches `jisuno.md` §8 exactly.

| Command | Key (display) | Key (file) | Hold | When | What it does |
|---|---|---|---|---|---|
| `voice.dictate` | Right ⌥ | `right_option` | yes | global | Hold, speak, release; text lands in the focused app |
| `voice.command` | Right ⌘ | `right_cmd` | yes | global | Hold, say a command phrase, release; Jisuno runs its steps |
| `model.warm` | ⌃⌥W | `ctrl+option+w` | no | global | Load the model ahead of time |
| `voice.hands_free_toggle` | ⌃⌥Space | `ctrl+option+space` | no | global | Start/stop hands-free dictation (§4) |
| `voice.cancel` | Esc | `escape` | no | `listening` | Cancel the current utterance; nothing inserted |
| `voice.cancel` | Esc | `escape` | no | `confirming` | Cancel the pending command; nothing runs |
| `voice.confirm` | Return | `return` | no | `confirming` | Run the pending command shown in the confirm prompt |
| `jisuno.settings` | ⌃⌥, | `ctrl+option+,` | no | global | Open Settings |

`model.unload` and `jisuno.menu` have no default key (§10).

## 4. Hold semantics

- **Key-down opens the mic.** Pressing a hold key opens the mic immediately, so no speech is lost. If the model is cold, model load starts at key-down and runs under speech; the HUD shows a thin load bar (§7 of `jisuno.md`). The HUD appears only once the hold passes the minimum (below).
- **A hold counts only if no other key is pressed during it.** If another key arrives mid-hold, the hold is abandoned silently: the mic closes, recorded audio is discarded, and the chord passes through to the app untouched. Example: Right ⌘+C types Cmd+C in the app; Jisuno does nothing.
- **First hold key wins.** Pressing the other hold key during a hold is ignored: it neither switches mode nor breaks the hold. Releasing the first hold key ends the utterance as usual.
- **Key-up ends and transcribes.** Releasing the hold key stops recording and transcribes.
- **Minimum hold: 150 ms (design value).** A release before 150 ms is a tap: the mic closes, the audio is discarded, a load started by this press is cancelled, no HUD, nothing transcribed.
- **Hands-free toggle.** ⌃⌥Space starts hands-free dictation without holding. It ends when silence lasts `hands_free_silence_ms` (default 500, from `settings.json`) or when the toggle is pressed again. Esc cancels it; nothing is inserted.
- **Cap: 2 min.** `dictation_limit_s` (default 120) caps dictation and hands-free alike. The HUD shows a countdown for the last 15 s before the cap (`0:15 left`, at 1:45 with the default); at the cap Jisuno stops and transcribes, as if released.
- **Esc and Return capture.** Esc is captured only in `listening`/`confirming` contexts; Return only in `confirming`. Everywhere else both pass through to the app untouched.

## 5. Contexts (`when`)

`when` is a string on the binding; absent means global.

| When | Meaning | Keys live |
|---|---|---|
| global (absent) | Anywhere, whenever Jisuno is running and Secure Input is off | Both hold keys, `model.warm`, `voice.hands_free_toggle`, `jisuno.settings`, and any user rebound global |
| `listening` | A hold utterance or hands-free session is recording | `voice.cancel` (Esc). Esc cancels; nothing inserted |
| `confirming` | A command with `confirm: true` is showing its HUD prompt (`Run "…" · Return to run · Esc to cancel`; Esc or 6 s cancels) | `voice.confirm` (Return runs), `voice.cancel` (Esc cancels) |

Esc in `confirming` cancels the pending command. Return outside `confirming` is never taken.

## 6. Key spelling grammar (file form)

- Form: lowercase, `+`-joined: modifiers first, then the key. Example: `ctrl+option+w`.
- Modifier order (fixed): `ctrl+option+shift+cmd`. Any other order is rejected (§8).
- Modifiers: `ctrl`, `option`, `shift`, `cmd`.
- Side-specific hold keys (the only legal `hold: true` values): `right_option`, `right_cmd`, `right_shift`, `right_ctrl`. There are no `left_*` spellings; left modifiers use the plain names.
- Key names: letters (`a`–`z`), digits (`0`–`9`), punctuation (`,`, `.`, `/`, `;`, `'`, `[`, `]`, `-`, `=`), `space`, `escape`, `return`, `tab`, `delete`, `f1`–`f20`.
- Case: always lowercase in the file. `Ctrl+Option+W` is rejected; write `ctrl+option+w`.

Display mapping (file → glyph):

| File | Display |
|---|---|
| `ctrl` | ⌃ |
| `option` | ⌥ |
| `shift` | ⇧ |
| `cmd` | ⌘ |
| `right_option` | Right ⌥ |
| `right_cmd` | Right ⌘ |
| `right_shift` | Right ⇧ |
| `right_ctrl` | Right ⌃ |
| `space` | Space |
| `escape` | Esc |
| `return` | Return |
| `tab` | Tab |
| `,` | , (shown after the chord: ⌃⌥,) |

## 7. Reserved and rejected keys

Rejected by validation with the reason and message. Nothing is written until all errors are fixed.

| Key | Reason | Error message |
|---|---|---|
| Fn / Globe | Apple dictation owns it | `Fn/Globe is reserved for Apple dictation · pick another key` |
| Left modifiers alone (`ctrl`, `option`, `shift`, `cmd` with `hold: true` on the left / without a side) | Left modifiers alone collide with everyday chords; holds must be right-side | `Hold keys must be a single right-side modifier (right_option, right_cmd, right_shift, right_ctrl)` |
| ⌘Space (`cmd+space`) | Spotlight | `cmd+space is a macOS system shortcut (Spotlight) · use another key` |
| ⌃Space (`ctrl+space`) | Input source switch | `ctrl+space is a macOS system shortcut (input source) · use another key` |
| ⌘Tab (`cmd+tab`) | App switcher | `cmd+tab is a macOS system shortcut (app switcher) · use another key` |
| ⌘Q (`cmd+q`) | Quit | `cmd+q quits the frontmost app · use another key` |
| ⌘H (`cmd+h`) | Hide | `cmd+h hides the frontmost app · use another key` |
| Plain Esc / Return without a `when` | Esc/Return must stay free for apps outside `listening`/`confirming` | `escape/return need a "when" (listening or confirming) · unscoped escape/return is rejected` |
| Plain letters without modifiers outside a `when` | Would steal typing | `"<key>" types text · add a modifier or a "when"` |
| Holding a non-modifier key (`hold: true` on anything but a right-side modifier) | Holds are modifiers only | `Only a single right-side modifier can be a hold key` |
| A hold binding that is a chord (`right_option+w` with `hold: true`) | A hold is one key alone | `A hold binding is one key alone · remove the chord or drop "hold"` |

## 8. Conflict detection and validation errors

- **Duplicate key+when (error).** Two bindings with the same key and the same `when` are an error; nothing is written.
  Message: `Duplicate binding: "<key>" (when: <when|global>) is already bound to "<command>"`.
- **Unknown command (error).** The command id is not in §10.
  Message: `Unknown command "<id>" · see the bindable commands list`.
- **Bad key spelling (error).** Unknown name, wrong modifier order, or uppercase.
  Messages: `Unknown key "<token>" in "<key>"`; `Modifiers must be in ctrl+option+shift+cmd order · got "<key>"`; `Keys are lowercase · got "<key>"`.
- **Hold on a non-right-side-modifier (error).**
  Message: `Hold keys must be a single right-side modifier (right_option, right_cmd, right_shift, right_ctrl)`.
- **Collision with a common app shortcut (warning, not error).** Allowed, but flagged in Settings with a warning badge next to the binding: Jisuno's global binding would take the chord from every app. Known list: ⌘W, ⌘S, ⌘Z, ⌘C, ⌘V, ⌘X, ⌘A, ⌘M, ⌘, (the system shortcuts of §7 are errors, not warnings).
  Message: `Warning: "<key>" is a common app shortcut (<name>) · apps will stop getting it`.
- **Secure Input note.** While Secure Input holds the keyboard, the event tap gets no keys and holds cannot fire. The menu bar icon shows the lock badge; the popover names the holding app; the HUD says `Secure input on (<app>) · keys paused` once per app per session. See `app-ui.md`. This is a runtime state, not a validation error.

Invalid files are rejected whole: last good config stays live, the HUD shows the error once on reload (e.g. `keybindings.json:12: unknown key "hald" · using last good config`), Settings shows the file row with Open file / Copy error.

## 9. Recording a key in Settings → Keybindings

- Click the key cell of a binding; it enters recording mode (`Press keys…`).
- Press the chord (or hold the single right-side modifier for a hold binding). The cell shows the display form live (⌃⌥⇧⌘, Right ⌥).
- Esc cancels recording; the old binding is kept.
- A conflict is shown inline in the row: `Taken by <command> (<when>)` with the clashing binding named; Replace and Cancel buttons. Replace rebinds and leaves the other command unbound.
- Save is immediate: the file is written atomically on every change and hot-reloaded. A failed write keeps the last good file and shows the §8 message.

## 10. Commands list

Every bindable command id with a one-line description.

| Command | Description |
|---|---|
| `voice.dictate` | Hold to dictate; release inserts the text |
| `voice.command` | Hold to speak a command phrase; release runs it |
| `voice.cancel` | Cancel the current utterance or pending confirm |
| `voice.confirm` | Run the pending confirmed command |
| `voice.hands_free_toggle` | Start/stop hands-free dictation |
| `model.warm` | Load the model ahead of time |
| `model.unload` | Unload the model now (no default key) |
| `jisuno.settings` | Open Settings |
| `jisuno.menu` | Open the menu bar popover (no default key) |

## 11. Changelog

- **2026-11-26, v1.** Right ⌥ (dictate) / Right ⌥+⇧ (command) held as the first default pair; ⌃⌥Space hands-free; Esc cancel; ⌃⌥, Settings.
- **2026-11-26, v2.** Right ⌘ replaces ⌥+⇧ for command: dictate = hold Right ⌥, command = hold Right ⌘. ⌃⌥W warms the model.
- **2026-11-26, round 4.** First hold key wins (the other hold key during a hold is ignored). `keybindings.json` becomes an object with `$schema` and `bindings`. Return confirm in `confirming`. 150 ms minimum hold; 2 min cap.
