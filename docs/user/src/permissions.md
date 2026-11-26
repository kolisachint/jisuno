# Permissions

Jisuno needs three macOS permissions. Grant them in **System Settings → Privacy & Security**.
On first run Jisuno checks each one and links to the right pane.

| Permission | Pane | Why Jisuno needs it |
|---|---|---|
| Microphone | Privacy & Security → Microphone | Captures your speech. The mic opens on key-down and closes when the utterance ends. |
| Accessibility | Privacy & Security → Accessibility | Inserts text into the focused app, presses buttons for command steps, and places windows. |
| Input Monitoring | Privacy & Security → Input Monitoring | Reads the hold keys and the Esc and Return keys, so holds, the hold-alone rule and cancel work. |

## What fails without each one

- **Microphone**: Jisuno cannot capture speech, so dictation and commands do nothing.
- **Accessibility**: text is not inserted, and command steps that touch windows or buttons fail.
  A tile step reports `tile failed: no permission`.
- **Input Monitoring**: the hold keys do nothing.

If you revoke a permission later, grant it again in the same pane.
