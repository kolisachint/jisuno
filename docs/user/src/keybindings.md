# Keybindings

The keys below are the defaults. They are stored in `~/.config/jisuno/keybindings.json`. The file
format and key spelling are in [Config files](config-files.md).

## Default keys

| Command | Key | File form | Hold | When | What it does |
|---|---|---|---|---|---|
| `voice.dictate` | Right ⌥ | `right_option` | yes | global | Hold, speak, release. The text goes into the focused app. |
| `voice.command` | Right ⌘ | `right_cmd` | yes | global | Hold, say a phrase, release. Jisuno runs its steps. |
| `model.warm` | ⌃⌥W | `ctrl+option+w` | no | global | Loads the model ahead of time. |
| `voice.hands_free_toggle` | ⌃⌥Space | `ctrl+option+space` | no | global | Starts or stops hands-free dictation. |
| `voice.cancel` | Esc | `escape` | no | listening | Cancels the current utterance. Nothing is inserted. |
| `voice.cancel` | Esc | `escape` | no | confirming | Cancels the pending command. |
| `voice.confirm` | Return | `return` | no | confirming | Runs the pending command. |
| `jisuno.settings` | ⌃⌥, | `ctrl+option+,` | no | global | Opens Settings. |

Two commands have no default key: `model.unload` and `jisuno.menu`. Bind them in the file if you want
a key.

"global" means the binding works anywhere while Jisuno runs. "listening" means it works only while
Jisuno is recording. "confirming" means it works only while a command waits for your confirmation.
Esc and Return are not taken from other apps outside those two states.

## Rules for hold keys

- A hold key works alone. If another key is pressed during the hold, the hold is dropped and the key
  combination goes to the app. Right Command + C copies in the app, and Jisuno does nothing.
- The first hold key wins. Pressing the other hold key during a hold is ignored.
- A release shorter than 150 ms is a tap, and nothing happens.

See [Dictation](dictation.md) for the details.

## Reserved keys

These keys cannot be bound. Jisuno rejects the file and keeps the last good one.

| Key | Why | Message |
|---|---|---|
| Fn / Globe | Apple dictation uses it. | `Fn/Globe is reserved for Apple dictation · pick another key` |
| A left modifier alone as a hold key | Left modifiers are used in everyday shortcuts. | `Hold keys must be a single right-side modifier (right_option, right_cmd, right_shift, right_ctrl)` |
| ⌘Space | Spotlight | `cmd+space is a macOS system shortcut (Spotlight) · use another key` |
| ⌃Space | Input source switch | `ctrl+space is a macOS system shortcut (input source) · use another key` |
| ⌘Tab | App switcher | `cmd+tab is a macOS system shortcut (app switcher) · use another key` |
| ⌘Q | Quits the app | `cmd+q quits the frontmost app · use another key` |
| ⌘H | Hides the app | `cmd+h hides the frontmost app · use another key` |
| Esc or Return with no `when` | They must stay free for other apps. | `escape/return need a "when" (listening or confirming) · unscoped escape/return is rejected` |
| A plain letter with no modifier and no `when` | It would stop you typing. | `"<key>" types text · add a modifier or a "when"` |
| Any other key as a hold key | Holds are modifiers only. | `Only a single right-side modifier can be a hold key` |
| A chord as a hold key, such as `right_option+w` | A hold is one key alone. | `A hold binding is one key alone · remove the chord or drop "hold"` |

Other file errors:

- Two bindings with the same key and the same `when`: `Duplicate binding: "<key>" (when: <when|global>) is already bound to "<command>"`.
- An unknown command ID: `Unknown command "<id>" · see the bindable commands list`.
- A bad key spelling, wrong modifier order, or uppercase: `Unknown key`, `Modifiers must be in ctrl+option+shift+cmd order`, or `Keys are lowercase`.

## Warnings

Some keys are common app shortcuts: ⌘W, ⌘S, ⌘Z, ⌘C, ⌘V, ⌘X, ⌘A, ⌘M and ⌘,. You can bind them. Jisuno
shows a warning badge next to the binding in Settings, because the binding takes the key from every
app:

```
Warning: "cmd+s" is a common app shortcut (Save) · apps will stop getting it
```

## Command IDs

| Command | What it does |
|---|---|
| `voice.dictate` | Hold to dictate. Release inserts the text. |
| `voice.command` | Hold to speak a command phrase. Release runs it. |
| `voice.cancel` | Cancels the current utterance or the pending confirmation. |
| `voice.confirm` | Runs the pending confirmed command. |
| `voice.hands_free_toggle` | Starts or stops hands-free dictation. |
| `model.warm` | Loads the model ahead of time. |
| `model.unload` | Unloads the model now. No default key. |
| `jisuno.settings` | Opens Settings. |
| `jisuno.menu` | Opens the menu bar popover. No default key. |

## Rebinding a key

Use either method. Changes take effect at once.

- **In Settings → Keybindings.** Click the key of a binding, press the new chord (or hold the
  right-side modifier for a hold key), and release. Press Esc to cancel and keep the old key. If the
  key is already used, the row names the other command. Choose Replace to move the key, or Cancel.
  Replace leaves the other command with no key.
- **In `keybindings.json`.** Edit the `key` value, or delete an entry to unbind its command. Save the
  file. An invalid file is rejected and the last good one stays live.
