# Permissions

Jisuno needs three macOS permissions. Grant them in **System Settings → Privacy & Security**.
The first-run Welcome window checks each one and links to the right pane.

| Permission | Pane | Why Jisuno needs it |
|---|---|---|
| Microphone | Privacy & Security → Microphone | Captures your speech. The mic opens on key-down and closes when the utterance ends. |
| Accessibility | Privacy & Security → Accessibility | Inserts text into the focused app, presses buttons for command steps, and places windows. |
| Input Monitoring | Privacy & Security → Input Monitoring | Reads the hold keys and the Esc and Return keys, so holds, the hold-alone rule and cancel work. |

## First run: the Welcome window

The Welcome window opens on the first run, when `settings.json` does not exist yet. It has three steps:

1. **Permissions.** One row per permission, checked live. A row shows `Allowed`, or `Not allowed` with
   an **Open System Settings** link. The window updates when you change a setting.
2. **Config files.** Jisuno writes the four config files to `~/.config/jisuno/`.
3. **Speech model.** A **Download** button for Parakeet TDT 0.6B v3, about 650 MB. Dictation works
   once the download finishes.

**Start Jisuno** is enabled when all three permissions are allowed. To open the window again later, use
**Welcome…** in the menu bar menu.

On the first run, Jisuno asks for Microphone and Accessibility access. It also asks for Input Monitoring.

## What fails without each one

- **Microphone**: Jisuno cannot capture speech, so dictation and commands do nothing. The HUD shows
  `Microphone access is off · open System Settings`.
- **Accessibility**: text is not inserted, and command steps that touch windows or buttons fail. The
  HUD shows `Can't insert: Accessibility is off · text in Recent`. A tile step reports
  `tile failed: no permission`.
- **Input Monitoring**: the hold keys do nothing. See below.

## Input Monitoring

Jisuno reads the keyboard through an event tap. Without Input Monitoring the tap cannot start.

- **At launch.** If the event tap cannot start, Jisuno exits with `grant Input Monitoring, then relaunch`.
  Grant the permission in System Settings, then open Jisuno again. If the tap starts but the permission
  is missing, the HUD shows an error at launch.
- **While running.** The menu bar menu shows `Input Monitoring — Not allowed`. If the key tap cannot be
  restarted, for example after you save `keybindings.json` while the permission is off, the HUD shows
  `Keys paused: Input Monitoring is off · using last good config`. Grant the permission and open Jisuno
  again.

## Changing a permission later

If you revoke a permission later, grant it again in the same pane. The menu bar menu shows the state of
each permission. Click a row to open its pane.

A rebuilt or re-signed copy of Jisuno can lose its permissions, because macOS ties them to the app's
signature. If the rows show `Not allowed` after an update, grant them again.
