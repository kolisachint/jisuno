# Protocols

Four wire formats: the stdout line protocol (CLI and `serve`), the helper process frames, the control socket
JSON, and the config files. The user-facing description of the first and the config files is in the user
guide ([Models](../../user/src/models.md), [Agents](../../user/src/agents.md),
[Config files](../../user/src/config-files.md)).

## Stdout line protocol

Source: `crates/jisuno-logic/src/protocol.rs`. Each event is one line on **stdout**, flushed at once. Logs and
download progress go to **stderr** and are not part of the protocol. A consumer reads stdout only.

Functions: `status`, `ready`, `level`, `phase`, `partial`, `final_text`, `segment`, `done`, `error`.

### `transcribe`

Mic mode:

```text
STATUS recording          capture started
SEGMENT hello             one chunk of text (may repeat)
STATUS transcribing       final decode running (after trailing silence)
SEGMENT world
DONE
```

With `--wav`, there is no `STATUS recording`. With no speech captured, the output is `STATUS transcribing`
then `DONE`, with no `SEGMENT`. Failure is `ERROR <message>` and exit code 1.

### `serve`

`serve` loads the model, prints `READY`, then reads commands on stdin.

| Stdin | Effect |
|---|---|
| `START` | Start one capture. The mic opens. |
| `CANCEL` | Stop the current capture and discard it. Ignored when idle. |
| `SHUTDOWN` | Exit. EOF on stdin also exits. |

Any other line is logged to stderr and ignored. Empty lines are ignored.

Stdout for one capture:

```text
STATUS listening          after START
LEVEL 0.0123              one per audio chunk (RMS)
PARTIAL hello wor         live text; each replaces the last
PHASE silence             trailing silence has just started
STATUS transcribing       after the silence timeout
FINAL hello world         the text to insert; may be empty
DONE
```

- `PARTIAL` is sent at most every `--partial-ms` (default 400), only after speech, and only when new audio
  has arrived. `serve` re-decodes all the audio so far for each one. `--partial-ms 0` turns them off.
- A `CANCEL` during the capture, or a capture with no speech, ends with `DONE` and no `FINAL`.
- `ERROR` during a capture ends that capture. The daemon keeps running. A load failure at startup prints
  `ERROR` and exits 1.
- `FINAL` is the only line to insert. `PARTIAL` is for display and must be overwritten, not appended.

The line types are defined in `protocol.rs`, the loop in `crates/jisuno/src/serve.rs`.

## Helper process (binary framing)

Source: `crates/jisuno-asr/src/worker/frame.rs` (framing and types), `server.rs` (child), `client.rs` (parent).
The child is `jisuno asr-worker`, started by the app when a model loads. It speaks on its stdin and stdout.
stderr carries logs.

### Frame

| Field | Size | Encoding |
|---|---|---|
| type | 1 byte | message type (below) |
| length | 4 bytes | payload length, little-endian `u32` |
| payload | `length` bytes | per message |

Maximum payload is `MAX_FRAME` = 64 MiB. Larger frames are rejected before allocation, on both ends. Audio
samples are `f32` little-endian, 16 kHz, mono. Clean EOF before a header means the child exited normally. A
truncated header or payload is an error.

Ids (`u64` little-endian) name a streaming utterance. One utterance runs at a time.

### Requests (parent to child)

| Type | Name | Payload | Reply |
|---|---|---|---|
| `0x01` | LOAD | model id, UTF-8 | `LOADED` or `LOAD_FAILED` |
| `0x02` | UNLOAD | none | `PONG`, then the child exits 0 |
| `0x10` | BEGIN | id | none |
| `0x11` | PUSH | id, then `f32` samples | `ERROR` (id and text) on failure |
| `0x12` | FINISH | id | `FINAL` (id and text) or `ERROR` |
| `0x13` | CANCEL | id | none |
| `0x14` | PARTIAL | id | `PARTIAL` (id and text) or `ERROR` |
| `0x20` | TRANSCRIBE | `f32` samples | `FINAL` with id `0`, or `ERROR` |
| `0x30` | STATUS | none | `STATUS` (JSON) |
| `0x31` | PING | none | `PONG` |

### Replies (child to parent)

| Type | Name | Payload |
|---|---|---|
| `0x81` | LOADED | load time in milliseconds, `u64` little-endian |
| `0x82` | LOAD_FAILED | error text, UTF-8 |
| `0x83` | FINAL | id, then UTF-8 text |
| `0x84` | PARTIAL | id, then UTF-8 text |
| `0x85` | STATUS | JSON: `{"state":"loaded"\|"idle","model":"<id>","rss_mb":<n>}` |
| `0x86` | PONG | none |
| `0x87` | ERROR | id, then UTF-8 text (id `0` for TRANSCRIBE) |

Requests are handled one at a time. Decoding is sequential, so the ONNX sessions are never used from two threads.
A `PUSH` never waits for a reply. The parent queues audio on a bounded channel and a writer thread sends it.

Streaming order for one dictation: `LOAD` (once), `BEGIN`, many `PUSH`, optional `PARTIAL`, `FINISH`, then
`UNLOAD` or the next `BEGIN`.

## Control socket

Source: `crates/jisuno/src/ipc.rs` (client and types), `crates/jisuno/src/app/socket.rs` (server).

- Path: `~/Library/Application Support/jisuno/jisuno.sock`. Mode `0600`. Removed on quit. A stale file (no
  listener) is replaced at start. If another instance answers, the second instance exits.
- One request per connection: a line of JSON, newline-terminated. Lines are cut at 4 KiB.
- Read and write timeout is 2 s on both ends.

| Request | Response |
|---|---|
| `{"cmd":"status"}` | `{"ok":true,"model":"<id>","state":"warm"\|"loading"\|"unloaded","rss_mb":<n>}` |
| `{"cmd":"warm"}` | `{"ok":true}`. Starts the load and returns before it finishes. |
| `{"cmd":"unload"}` | `{"ok":true}`, or `{"ok":false,"error":"busy"}` while recording or transcribing. |

Errors: `{"ok":false,"error":"model not installed"}` for `warm` without the model files, and
`{"ok":false,"error":"bad request: …"}` for a line that does not parse. A client that cannot connect gets
`error: Jisuno is not running` on stderr and exit code 1 from `jisuno model`.

`warm` and `unload` are queued to the tick (`HookRequest`), so the model state changes on the main thread.

## Config files

| File | Schema | Defaults | Notes |
|---|---|---|---|
| `settings.json` | `crates/jisuno-logic/schemas/settings.schema.json` | `crates/jisuno-logic/defaults/settings.json` | `deny_unknown_fields`: an unknown key rejects the file. |
| `keybindings.json` | `schemas/keybindings.schema.json` | `defaults/keybindings.json` | `{"$schema", "bindings": [...]}`. |
| `theme.json` | `schemas/theme.schema.json` | `defaults/theme.json` | Every key is optional. |
| `voice-control-config.json` | `schemas/voice-control-config.schema.json` | `defaults/voice-control-config.json` | `version` (must be `1`) and `commands`. Validated by `jisuno-logic/src/voice_control/`. |

The first run writes the defaults and the schemas (`embedded_app_files` in `app/config.rs`). The schema files
are embedded in the binary. Keys, values and the user meaning are in the
[config file reference](../../user/src/config-files.md). The voice-command format is in
`docs/design/voice-control.md` §3, and the validation rules are in §7.

Parse errors name the file, the line and the problem, for example `keybindings.json:12: unknown key "hald"`.
The parsers keep line numbers, so `jisuno control validate` and the HUD report the same line.
