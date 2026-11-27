# FAQ

## Privacy and network

**Does Jisuno send my voice anywhere?**
No. Recognition runs on your Mac. Audio stays in memory while you hold the key and is dropped after it is
transcribed. Jisuno never writes audio to disk.

**Is there telemetry or an update check?**
No. The only network use is downloading a model. Updates are manual.

**What does Jisuno keep, and where?**

| Data | Where | Lifetime |
|---|---|---|
| Settings, keybindings, theme, voice commands | `~/.config/jisuno/` | until you delete them |
| Models | `~/Library/Application Support/jisuno/models/` | until you remove them |
| Recent (last 5 results) | memory | until you quit |
| Benchmark results | `~/Library/Caches/jisuno/bench/` | until you delete them |
| Logs | stderr of the app, when you run it from a terminal | no file is written |

Logs hold counts, not speech. `JISUNO_LOG_TEXT=1` adds the text, for debugging only.

## Offline

**Does it work offline?**
Yes, once the model is installed. Dictation and commands need no network.

**Do I need an account?**
No.

## Models

**Which model should I use?**
The default, Parakeet TDT 0.6B v3 int8, handles several languages and detects the language of each
utterance. Use v2 (English only) only if you want that model. Both are about 630 to 650 MB.

**Can I use another model?**
Not in this version. The registry has two ids: `parakeet-tdt-0.6b-v3-int8` and `parakeet-tdt-0.6b-v2-int8`.

**Why is the first dictation slow?**
The model loads on demand. Press `⌃⌥W` after login, or set `model_load` to `at_login`. See [Models](models.md).

## Memory

**How much memory does it use?**
With the model loaded, about 1.1 GB, in a helper process (`jisuno asr-worker`). With the model unloaded, the app
uses 32 to 39 MB.

**Why is it unloaded most of the time?**
On an 8 GB Mac, 1.1 GB is more than 10% of the RAM. The model unloads after 10 minutes without use
(`unload_after_idle_min`). Set it to `0` to keep the model loaded. `jisuno model unload` unloads it now.

## Keys

**Why Right ⌥ and Right ⌘?**
The right-side modifiers are free on a default macOS setup when they are held alone. The left modifiers are used
in everyday shortcuts such as ⌥ with a letter or ⌘C. Fn and Globe belong to Apple dictation, so Jisuno cannot use
them.

**Will Right ⌘ + C still copy?**
Yes. A hold counts only if no other key is pressed during it. Right ⌘ + C goes to the app, and Jisuno does
nothing.

**Can I change the keys?**
Yes. Holds must be right-side modifiers. Use **Settings → Keybindings** or edit `keybindings.json`. See
[Keybindings](keybindings.md).

**Why does nothing happen when I hold a key?**
Check the HUD and the menu bar. A lock badge means Secure Input is on. Input Monitoring must be allowed. See
[Troubleshooting](troubleshooting.md#keys-do-nothing).

## Secure Input

**What is Secure Input?**
A password field, or an app that asks macOS to keep the keyboard private, blocks other apps from seeing key
events. Terminal does this when **Secure Keyboard Entry** is on. Jisuno cannot see your keys then, so it pauses.

**What do I see?**
The HUD shows `Secure input on (<app>) · keys paused`, and the menu bar icon shows a lock badge. The popover names
the app. Jisuno resumes when the field closes or the app stops holding the keyboard.

## Permissions

**Why three permissions?**
Microphone for capture. Accessibility to insert text and run window and button steps. Input Monitoring to read
the hold keys. See [Permissions](permissions.md).

**Why does macOS ask again after an update?**
macOS ties the permissions to the app's signature. A new or re-signed build is a new identity. Grant the
permissions again in the same panes.

## Dictation

**Can I say "comma" or "new line"?**
No. Spoken punctuation is not in this version. The model's own punctuation and casing are used.

**Can I edit text by voice ("delete that")?**
No.

**Why did my text go to Recent instead of the app?**
Either the app that had focus at key-down quit or closed its window before key-up (`Focus lost`), or
Accessibility is off (`Can't insert`). Nothing is typed anywhere else. Copy the text from Recent.

**What if an app ignores paste?**
Add its bundle ID to `insert_type_apps` in `settings.json`, or set **Settings → General → Insert text by** to
Type. The text is then typed as key events.

**How long can I dictate?**
Two minutes, hands-free included (`dictation_limit_s`). The HUD counts down the last 15 seconds.

## Commands

**Does Jisuno use an AI model to understand commands?**
No. Commands are fixed phrases with optional `{slot}` values, matched against `voice-control-config.json`. The
matcher reads only the file. It does not read other apps' interfaces when you speak.

**Can an AI agent add commands?**
Yes, with `jisuno control add`. The command is checked before it is written, and it is tagged `"agent": true`.
See [Agents](agents.md).

## Install and removal

**Why does macOS block the app?**
An unsigned build is blocked by Gatekeeper. Open **System Settings → Privacy & Security** and choose **Open
Anyway**. See [Install](install.md).

**How do I update?**
Run `brew upgrade --cask jisuno`, or download the new zip.

**How do I uninstall it completely?**
Turn off **Launch at login** in Settings → General first. Then delete `Jisuno.app` and these folders:

```text
~/Library/Application Support/jisuno     models (about 650 MB or more)
~/.config/jisuno                         settings and voice commands
~/Library/Caches/jisuno                  benchmark results
~/Library/Preferences/dev.kolisachint.jisuno.plist
```

The Homebrew cask lists the same paths under `zap`.
