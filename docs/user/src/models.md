# Models

Speech recognition runs on a local model. Jisuno uses Parakeet TDT 0.6B, in ONNX format with int8
weights. The model weights are NVIDIA's, redistributed as ONNX exports under CC-BY-4.0.

## Which model

| Model | Settings name | Size | Languages |
|---|---|---|---|
| Parakeet TDT 0.6B v3 int8 (default) | `parakeet-tdt-0.6b-v3-int8` | about 650 MB | Multilingual. The model detects the language of each utterance. |
| Parakeet TDT 0.6B v2 int8 (optional) | `parakeet-tdt-0.6b-v2-int8` | about 630 MB | English only. |

Choose the model in **Settings → Model**, or set `model` in `settings.json`. Only one model is
loaded at a time.

On the command line:

```sh
jisuno setup                                        # download the default model (v3)
jisuno setup --model parakeet-tdt-0.6b-v2-int8      # download the English model
jisuno models                                       # list the models and whether they are installed
jisuno models download <id>                         # (re)download one model, e.g. after deleting it
jisuno models verify <id>                           # check installed files against their pinned hashes
jisuno models remove <id>                           # delete an installed model
```

`<id>` is the registry id: `parakeet-tdt-0.6b-v3-int8` or `parakeet-tdt-0.6b-v2-int8`.

`setup` and `models download` take `--ca-cert <file.pem>` (an extra trusted CA, repeatable) for a
network with its own certificate authority. `--insecure` turns TLS verification off. Use it only as
a last resort on a network you trust.

## Robust downloads

Every model file carries a pinned sha256 and byte size. `jisuno setup` and
`jisuno models download` fetch each file from HuggingFace, and fall back to the
`kolisachint/jisuno` `models-v1` release mirror if HuggingFace fails. Each file is verified for size
and hash. Installs are atomic: files land in a staging directory first and are renamed into place
only once everything verifies, so an interrupted download never leaves a half model behind.

Stopping mid-download keeps `.part` files. The next run resumes them with an HTTP Range request when
the server supports it. A lock file keeps the app and the CLI from downloading the same model at the
same time. `models remove` refuses while a download holds the lock.

`SSL_CERT_FILE` adds a PEM bundle to the trusted roots for downloads.

## Where models are stored

Models live in the platform data directory: `~/Library/Application Support/jisuno/models/` on macOS.
Set `JISUNO_MODELS_DIR` to put them somewhere else. Each model is one folder named by its id.

## When the model is in memory

The model moves through these states: unloaded, loading, warm, and back to unloaded after it has been
idle.

- **On demand (default).** Nothing is in memory at login. Holding a dictation key while the model is
  unloaded starts the load at key-down. The load runs under your speech, and the HUD shows a thin
  load bar. Only the part of the load still running at key-up adds to the wait. Live partial text
  starts once the model is warm.
- **Warm key.** `⌃⌥W` (`model.warm`) loads the model ahead of time. The default commands also
  include "warm up".
- **At login.** Set `model_load` to `at_login` to load the model when you log in.
- **Idle unload.** `unload_after_idle_min` (default `10`) unloads the model after that many minutes
  without use. `0` means never. `model.unload` unloads it at once. It has no default key. Bind it in
  [Keybindings](keybindings.md), or run `jisuno model unload`.

The model runs in a helper process, `jisuno asr-worker`, that the app starts when the model loads and
stops when it unloads. Unloading exits that process, so the operating system takes back all of the
model's memory. The app itself stays small while the model is unloaded (about 32 to 39 MB).

Use `jisuno model status`, `jisuno model warm` and `jisuno model unload` to check and change the state
from the command line. The app must be running. See [Agents](agents.md#model-commands).

## Memory and speed

A warm model uses about 1.1 GB of memory, in the helper process. On an 8 GB Mac that is more than 10%
of the RAM, so loading on demand and unloading when idle are the defaults.

The speed target is key-up to text in 150 ms or less, with a warm model, on the reference machine: a
MacBook Air M1 with 8 GB of RAM. Measured on that machine, key-up to text is about 21 ms when you pause
while speaking. A long burst with no pause takes longer, because the whole tail is decoded at key-up.
Settings → Performance shows the targets. To measure your own Mac, run the benchmark from a checkout:
`cargo run --release -p jisuno-bench`. The **Run benchmark** button in Settings looks for a
`jisuno-bench` binary next to the app, and the release app does not include one, so it reports an
error there.

## Check the model from the command line

```sh
jisuno transcribe                     # speak; stops after about 600 ms of silence
jisuno transcribe --silence-ms 800    # a longer pause before it stops
jisuno transcribe --wav recording.wav # transcribe a WAV file instead of the mic
```

`transcribe` loads the model for that one run and exits. It does not use the helper process.

## Network use

Downloading the model is the only network use in Jisuno. Recognition runs on your Mac. See
[Introduction](introduction.md).
