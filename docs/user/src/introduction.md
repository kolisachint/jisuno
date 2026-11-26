# Introduction

Jisuno is a voice tool for macOS. It lives in the menu bar and is one key away.

- **Dictate.** Hold Right Option, speak, release. The text goes into the app that has focus.
- **Command.** Hold Right Command, say a phrase such as "start working" or "focus terminal",
  release. Jisuno runs the steps you set for that phrase.

A small HUD near the notch (or by the cursor, or off) shows the mode, the mic level, live
partial text, model loading, and a one-line result. Short tones mark start, stop and cancel.

## Local and offline

Speech recognition runs on your Mac with the Parakeet TDT 0.6B model (ONNX). Audio is not sent
anywhere. The model loads when you first need it and unloads after 10 minutes of no use by
default. See [Models](models.md).

## Privacy

- The last 5 transcripts are kept in memory only, as Recent. They are gone when you quit.
- There is no telemetry.
- The only network use is downloading the model. There is no update check. Updates are manual.

## Limits in version 1

- macOS only.
- No wake word. The mic opens only while you hold a key or a hands-free session is running.
- No spoken punctuation and no voice editing ("delete that"). The model's own punctuation and
  casing are used as they come.

Next: [Install](install.md), then [Permissions](permissions.md).
