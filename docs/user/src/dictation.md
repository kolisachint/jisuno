# Dictation

## Dictate with a hold

1. Hold **Right Option** (the Option key on the right side of the keyboard).
2. Speak.
3. Release. Jisuno transcribes and inserts the text into the app that had focus when you pressed
   the key.

The mic opens as soon as you press the key, so the first words are not lost. If the model is not
loaded, loading starts at key-down and runs while you speak. The HUD shows a thin load bar. See
[Models](models.md).

## Hold rules

- **Minimum 150 ms.** A release sooner than 150 ms is a tap. The audio is discarded, nothing is
  transcribed, no HUD appears, and a model load started by that press is cancelled.
- **Hold alone.** A hold counts only if no other key is pressed during it. If you press another
  key, the hold is dropped: the audio is discarded and the key combination reaches the app as
  usual. For example, Right Command + C copies in the app, and Jisuno does nothing.
- **First hold key wins.** While Right Option is held, pressing Right Command does not switch
  mode and does not end the hold. Releasing Right Option ends the utterance.

Hold keys are the right-side Option and Command keys by default. You can rebind them. See
[Keybindings](keybindings.md).

## Hands-free

Press **⌃⌥Space** (Control, Option, Space) to start hands-free dictation without holding a key.

- It ends after 500 ms of silence (`hands_free_silence_ms`), or when you press ⌃⌥Space again.
- Press **Esc** to cancel. Nothing is inserted.

## Length limit

Dictation, hands-free included, is capped at 2 minutes (`dictation_limit_s`, default `120`).
The HUD counts down the last 15 seconds (`0:15 left`). At the cap, Jisuno stops and transcribes,
as if you had released the key.

## Cancel

Press **Esc** while Jisuno is listening. The utterance is discarded and nothing is inserted. Esc
is taken from the app only while Jisuno is listening or waiting for a command confirmation.
Otherwise the app gets Esc as usual.

## Where the text goes

- **Paste (default).** Jisuno saves the clipboard, pastes the text, and restores the clipboard
  (`restore_clipboard`, default on).
- **Typing.** Jisuno types the text as key events. It does this for apps listed in
  `insert_type_apps`, and when the clipboard write fails.
- **Focus lost.** The text goes to the app that had focus at key-down. If that app quit or its
  window closed before key-up, nothing is inserted anywhere else. The text stays in Recent, and
  the HUD says `Focus lost: text in Recent`.

## Recent

The last 5 transcripts are listed in Recent in the menu bar popover. They are held in memory only
and are gone when you quit.

## Secure input

A password field, or an app that holds Secure Event Input, blocks keyboard events. While this is
on, holds cannot fire.

- The menu bar icon shows a lock badge, and the popover names the app that holds the keyboard.
- The HUD says `Secure input on (<app>) · keys paused`, once per app per session.

## Microphone

Choose the mic in **Settings → Audio**. The default is the system input (`input_device`:
`system`). A named device must match the name macOS shows for it. Jisuno opens the device when a
hold starts.

- **Bluetooth.** If the input's name contains "Bluetooth", "AirPods" or "Beats", Jisuno shows a
  warning: audio is compressed and latency is higher. Use the built-in microphone for dictation. The
  HUD shows `Bluetooth mic: lower quality, more latency` once for each device in a session.
- **Missing device.** If the chosen device is not connected, Jisuno uses the system input. The HUD
  shows `Mic "<name>" not found · using system input`.

## Live partials

With `partials` set to `true` (default), the HUD shows the text as you speak, once the model is warm.
This text is only a preview. It is replaced as Jisuno hears more, and only the final text is
inserted. Set `partials` to `false` in **Settings → General** to see only the level and the final
result.

## HUD and sounds

- Where the HUD appears is set by `hud`: `notch` (default), `cursor` or `off`. Set it in **Settings →
  General**.
- Short tones play on start, stop and cancel when `sounds` is `true` (default). Dictation and command
  mode use different tones.

## What dictation does not do

Jisuno does not turn spoken words into punctuation ("comma", "new line") and does not edit text
by voice ("delete that"). The model's own punctuation and casing are used.
