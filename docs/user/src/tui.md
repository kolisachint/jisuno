# Use from a TUI

`jisuno serve` is a background process that loads the models once and then waits for commands on
stdin. A terminal app can use it for push-to-talk input. Because the models stay loaded, a press does
not wait for a model load after the first `READY`.

`serve` transcribes live. It streams `PARTIAL` lines while you speak and sends one `FINAL` line at
the end. The app can show the text as it builds, then insert only the final text.

The reference integration is for hoocode, a terminal coding agent. It wires push-to-talk to `ctrl+r`.
Copy the files below into hoocode.

## serve: stdin and stdout

Start it with:

```sh
jisuno serve
```

Flags (defaults shown):

- `--model`: the model to use. Default `parakeet-tdt-0.6b-v3-int8`.
- `--silence-ms 600`: trailing silence in milliseconds that ends a capture.
- `--partial-ms 400`: minimum gap between `PARTIAL` lines. `0` turns partials off, so you get only
  `FINAL`.

The environment variables `JISUNO_MODEL`, `JISUNO_SILENCE_MS` and `JISUNO_PARTIAL_MS` set the same
values. A flag overrides its variable.

`serve` loads the model into its own process and keeps it until it exits. Each `PARTIAL` decodes all
the audio captured so far. `FINAL` decodes the whole capture once more. The menu bar app is separate:
it streams partials with a helper process and does not use `serve`.

Commands on stdin, one per line:

```
START        begin mic capture and voice detection
CANCEL       stop the current capture without transcribing
SHUTDOWN     exit
```

One capture runs at a time. The mic opens on `START` and closes when the capture stops.

Every line on stdout is one event:

```
READY                    # models loaded; ready for START
STATUS listening         # capture started
LEVEL 0.0123             # mic level for one audio chunk, while listening
PARTIAL hello wor        # interim text; replaces the previous PARTIAL
PHASE silence            # trailing silence started, while listening
STATUS transcribing      # final decode running
FINAL hello world        # committed text; DONE follows
DONE                     # capture finished
ERROR <message>          # fatal
```

`PARTIAL` is a best guess that changes while you speak. Overwrite it, do not append. `FINAL` is the
text to insert.

Each line is one event. Read stdout only. Stderr carries logs and is not part of the protocol.

The command `jisuno transcribe` uses an older form of the protocol for one-off capture. It writes
`STATUS recording`, `SEGMENT <text>` lines, then `DONE`. Use it only where `serve` is not available.

## hoocode integration

The integration has four parts.

### 1. `packages/tui/src/voice-transcribe.ts` (new file)

Copy [`voice-transcribe.ts`](https://github.com/kolisachint/jisuno/blob/main/docs/user/src/tui/voice-transcribe.ts)
into `packages/tui/src/` (also in this repo at `docs/user/src/tui/voice-transcribe.ts`). It exports:

- `VoiceDaemon`. Starts `jisuno serve` once and keeps it running across presses. Restarts it if it
  crashes. Writes `START` and `CANCEL` to its stdin. It checks support with `jisuno serve --help`
  (exit `0` means `serve` exists, whatever the model load time). If `serve` is missing, it falls back
  to running `jisuno transcribe` for each press.
- Only `FINAL` is typed into the input, as a bracketed paste. `PARTIAL` text is shown but never
  committed, because it changes.
- `VoicePanel`. Turns the events into a live multi-line panel: a spinner while the model warms up, a
  `Listening` header with a level meter, the transcript building on the next line, a shrinking
  silence countdown once trailing silence starts, and a dim `esc cancel` footer. It is empty on
  `DONE` or `ERROR`.

### 2. `packages/tui/src/keybindings.ts` (add one binding)

```ts
// In `interface Keybindings`:
"tui.input.voiceTranscribe": true;

// In `TUI_KEYBINDINGS`:
"tui.input.voiceTranscribe": { defaultKeys: "ctrl+r", description: "Voice input" },
```

### 3. `packages/tui/src/tui.ts` (wire the daemon and draw the panel)

```ts
import { VoiceDaemon, VoicePanel } from "./voice-transcribe.js";
import { resolveJisunoBin } from "@hoocode/coding-agent/config";

// Created once for the TUI process lifetime (not per keypress).
const voicePanel = new VoicePanel();
const voiceDaemon = new VoiceDaemon(resolveJisunoBin(), stdinBuffer, (event) => {
  voicePanel.handle(event);
  if (event.type === "error") ctx.ui.notify(event.message, "error");
  ctx.ui.setVoicePanel(voicePanel.render()); // string[]; [] once collapsed
});

let voiceSession: { cancel(): void } | null = null;

// Inside the keypress handler:
if (keybindings.matches(data, "tui.input.voiceTranscribe")) {
  voiceSession = await voiceDaemon.startCapture();
  return;
}
// A second binding (e.g. `esc`) cancels the in-flight capture:
if (keybindings.matches(data, "tui.input.voiceCancel")) {
  voiceSession?.cancel();
  return;
}

// On TUI shutdown:
voiceDaemon.dispose();
```

`VoicePanel.render()` returns the panel as lines. Draw them in a small region above the input. While
`voicePanel.active` is true, redraw that region about every 80 ms so the spinner, meter and countdown
move. Clear the region when `render()` returns `[]`.

The `esc cancel` hint is in the panel's footer, so the global key-hints row needs no change.

### 4. `packages/coding-agent/src/config.ts` (find the binary)

```ts
import { existsSync } from "node:fs";
import { join } from "node:path";

export function resolveJisunoBin(): string {
  if (process.env.JISUNO_BIN) return process.env.JISUNO_BIN;
  const bundled = join(getHooCodeDir(), "bin", "jisuno");
  if (existsSync(bundled)) return bundled;
  return "jisuno"; // fall through to PATH
}
```

`JISUNO_BIN` points at a specific binary. Otherwise the bundled `bin/jisuno` is used, then `jisuno` on
`PATH`.

## Notes

- The TUI reads only the stdout protocol. Keep that contract as it is.
- The client's own silence countdown uses 600 ms, which matches `serve`'s default `--silence-ms`. It is
  not the 500 ms hands-free setting from the desktop app.
- `VoiceDaemon.dispose()` sends `SHUTDOWN`, waits briefly, then kills the process. Call it once, when
  the TUI exits, not for each press.
