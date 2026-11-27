# Troubleshooting

Find the HUD message or symptom below. The HUD shows the message for a few seconds. The menu bar
popover shows the current state.

## Keys do nothing

**`Secure input on (<app>) · keys paused`.** A password field, or an app holding Secure Event Input,
is blocking the keyboard. Holds cannot fire until it stops. The menu bar icon shows a lock badge, and
the popover names the app. Close the password field or quit the app that holds the keyboard. Terminal
holds it when **Secure Keyboard Entry** is on. Jisuno resumes by itself.

**`Keys paused: Input Monitoring is off · using last good config`.** Jisuno could not restart its key
tap, usually after you saved `keybindings.json` without Input Monitoring. Grant Input Monitoring in
System Settings → Privacy & Security, then open Jisuno again. See [Permissions](permissions.md).

**Jisuno exits when it opens** with `grant Input Monitoring, then relaunch` in the log. The event tap
could not start. Grant Input Monitoring, then open Jisuno again.

**The menu shows a permission as `Not allowed`.** Click the row to open its System Settings pane. See
[Permissions](permissions.md).

## Text does not appear

**`Focus lost: text in Recent`.** The app that had focus at key-down quit, or its window closed, before
key-up. Nothing was typed anywhere else. Open the menu bar popover and click the row in **Recent** to
copy the text.

**`Can't insert: Accessibility is off · text in Recent`.** Grant Accessibility. The text is kept in
Recent, so copy it from there.

## The model

**`Model not downloaded · open Settings → Model`.** The model is not on disk. Open **Settings → Model**
and choose **Download**, or run `jisuno setup`. Dictation works once the download finishes.

**`jisuno transcribe` says `no model found for '<id>'`.** Run `jisuno setup --model <id>`. To see
what is installed, run `jisuno models`.

**The download fails or stalls.** Run the same command again. A partial download resumes. If the
network uses its own certificate authority, pass `--ca-cert <file.pem>` or set `SSL_CERT_FILE`. If
`models verify <id>` reports a bad file, run `jisuno models remove <id>` and download it again. See
[Models](models.md).

**`<id> is busy (download in progress, pid N)`.** Another download is running, from the app or the
CLI. Wait for it to finish, then try again.

**The model stays in memory.** It unloads after `unload_after_idle_min` minutes without use (default
10). Set it to `0` to keep it loaded. `jisuno model unload` unloads it now, unless Jisuno is recording
or transcribing.

**The first dictation is slow.** The model loads when you first hold a key, or when you press `⌃⌥W`.
Press `⌃⌥W` once after login to load it ahead of time, or set `model_load` to `at_login`.

**`jisuno model status` says `error: Jisuno is not running`.** The app has to be open. The model
commands talk to the app.

## Microphone

**`No microphone found · dictation off`.** Connect a microphone, or choose one in **Settings → Audio**.

**`Microphone access is off · open System Settings`.** Grant Microphone access. See
[Permissions](permissions.md).

**`Mic "<name>" not found · using system input`.** The device set in `input_device` is not connected.
Jisuno uses the system input. Choose the device again in **Settings → Audio** when it is back.

**`Bluetooth mic: lower quality, more latency`.** The input is a Bluetooth device. Use the built-in
microphone for dictation. See [Dictation](dictation.md).

## Commands

**`Did you mean "<phrase>"?` or `No command matched`.** Nothing ran. Say the phrase again, or check
the list with `jisuno control list`.

**`Step 2 of 3 failed: <action> · <reason>`.** The command stopped at that step, and the steps before it
ran and are not undone. The reason names the cause. Common reasons are listed in
[Voice commands](commands.md#when-a-step-fails).

**`Not run`.** You did not confirm a command within 6 seconds, so it was cancelled.

## Config files

**`<file>:<line>: <problem> · using last good config`.** The file has an error. Jisuno keeps the last
good version. Fix the line and save the file. `jisuno control validate` prints the same errors.

**A setting has no effect.** Check the key name and value in [Config files](config-files.md). Unknown
keys are rejected, so the whole file falls back to the last good version. A `settings.json` that still
has `language` is rejected for this reason. Delete the key.

## Display

**No HUD appears.** Check **Settings → General → Listening cue**. `Off` hides the HUD. Failures still show in
the popover and on the menu bar icon. With `Notch`, the HUD sits under the menu bar, centred, on a Mac without
a notch. It does not take clicks, so a click on that area reaches the window below.

## Command-line tools

**`no voice-control-config.json at <path> (run the app once or create it)`.** `jisuno control` needs the file.
Open the app once, which writes the defaults, or check `JISUNO_CONFIG_DIR`.

**`error: busy` from `jisuno model unload`.** Jisuno is recording or transcribing, and unloading now would lose
the audio. Try again when it is idle.

**`error: model not installed` from `jisuno model warm`.** Download the model first. See [Models](models.md).

## Logs

Jisuno writes timestamped log lines to stderr. It does not write a log file. To read them, run the app binary from a terminal:

```sh
/Applications/Jisuno.app/Contents/MacOS/jisuno
```

Logs contain counts, not speech. Set `JISUNO_LOG_TEXT=1` to include the dictated and partial text in
the logs. Use it only while you debug, and remove it afterwards.
