# Bench fixtures

A fixture is a pair of files sharing a stem:

- `name.wav` — any rate, channel count, or PCM format `hound` reads; the
  harness loads it as 16 kHz mono, exactly like `--wav` transcription.
- `name.txt` — the expected transcript, plain text.

The harness decodes every `*.wav` with a sibling `*.txt` and reports word
error rate per pair plus the mean. A `*.wav` without a matching `*.txt` is
used for timing clips only (the first sorted `*.wav` supplies the 2/5/15 s
clips by slicing or looping); it is skipped for WER.

With no fixtures directory (or no pairs), the harness decodes deterministic
speech-free noise instead and says so in its output — decode time is still
meaningful, WER is reported as `n/a`.

Do not commit audio or transcripts here; fixtures stay local (owner-recorded
set, LibriSpeech excerpts). To make one on macOS:

```sh
say -o /tmp/b.aiff "the quick brown fox jumps over the lazy dog" && \
  afconvert -f WAVE -d LEI16@16000 /tmp/b.aiff /tmp/fixtures/say.wav
echo "the quick brown fox jumps over the lazy dog" > /tmp/fixtures/say.txt
cargo run --release -p jisuno-bench -- --fixtures-dir /tmp/fixtures
```
