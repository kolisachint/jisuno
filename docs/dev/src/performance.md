# Performance

Full method, tables and rejected experiments: [`crates/jisuno-asr/PERF.md`](../../../crates/jisuno-asr/PERF.md).
This page is the summary. Keep the numbers in the user guide and README in step with it.

Reference machine: MacBook Air M1 (MacBookAir10,1), 4 performance and 4 efficiency cores, 8 GB RAM. Model:
`parakeet-tdt-0.6b-v3-int8`, release build.

## Targets

| Metric | Target | Source |
|---|---|---|
| Key-up → text, warm model | ≤ 150 ms | `docs/design/jisuno.md` §6 |
| Final decode, 5 s clip, warm | ≤ 100 ms | §6; checked by `jisuno-bench` |
| Cold load | ≤ 1.5 s | §6.1, round 6 item 3 |
| Memory, warm (helper process) | < 1.2 GB | round 9 item 2. Supersedes "< 1 GB" in round 6 item 3. |
| Memory, unloaded (app) | < 60 MB | §6.1, round 6 item 3 |
| Step budget (warm) | flush ≤ 15 ms, post-process ≤ 2 ms, insert ≤ 10 ms | §6 table |

## Measured

| Measure | Result | Target | Status |
|---|---|---|---|
| Key-up → text, pauses in speech (app, streaming, live) | 21–22 ms median (7.0 s and 17.4 s clips) | ≤ 150 ms | met |
| Key-up → text, no pause (5.8 s clip, app) | 162 ms | ≤ 150 ms | not met (marginal) |
| Full-clip decode at key-up, 5 s, no pause (bench) | 360 ms median | ≤ 150 ms | not met |
| Warm decode, 5 s clip (full clip, bench) | 378 ms median (351 ms in experiment D, before the arena change) | ≤ 100 ms | not met |
| Cold load | 1156 ms (bench, final config); about 1.26 s in the helper | ≤ 1.5 s | met |
| Memory, warm, helper process | 1079 MiB loaded, 1116 MiB after one decode (`footprint`); 1008 MiB RSS in the final bench config | < 1.2 GB | met under 1.2 GB. The later measurement, 1120 MiB, misses the round 6 target of 1 GB. |
| Memory, unloaded, app | 11 MiB for the driver process with the model unloaded (helper experiment). The 32–39 MB app figure is in design round 9 and is not in `PERF.md`. | < 60 MB | met |
| WER, five fixtures, full clip | 0.336 mean (baseline and final) | no worse | met |
| Streaming WER vs full clip | 0.3857 vs 0.3875 (−0.002) on nine pairs | ≤ full + 0.01 | met |
| Determinism | 10 runs over five fixtures identical | identical | met |

Read the first row with care. It is the pause-split path, and it depends on tail length: the tail decoded
at key-up is the audio since the last pause. A long burst with no pause decodes in full at key-up, which is
the 162 ms row. The 5 s warm decode and the no-pause key-up miss their targets. `PERF.md` names the cause:
the encoder forward pass, about 70 ms per second of audio.

The "memory warm" number changes with the method. `PERF.md` reports RSS in the experiment table and
`phys_footprint` (`footprint`) in the memory section. Use the same metric when comparing two runs.

## How the numbers were measured

| Measure | How |
|---|---|
| Warm decode, key-up (full clip), cold load, memory | `cargo run --release -p jisuno-bench -- --fixtures-dir <dir> --runs 5 --warmup 1` (see [Testing](testing.md#bench)) |
| Streaming push and `finish()` | `jisuno-bench --streaming --stream-config "0.2,1.0,8.0"` |
| Key-up → text in the app | run the app with stderr to a file and read `key-up -> text: N ms (finish M ms, child X MB)` |
| Helper memory | `footprint <helper pid>` after load and after decodes, or `ps` sampling (`PERF.md`, "Memory") |
| Per-stage timings | `JISUNO_PROFILE=1 jisuno transcribe --wav <file>` |

Measure on the reference machine, with the machine quiet. Other work on the same Mac changes the medians.

The Performance pane in Settings runs `jisuno-bench` with no fixtures. Its decode times are real. Its
WER is `n/a`.

## Experiments

Kept:

| Change | Effect | Where |
|---|---|---|
| ONNX intra-op threads 2, deterministic compute | 5 s decode 573 → 351 ms; 15 s 1692 → 1014 ms | `PERF.md` experiment 3 |
| CPU memory arena off | warm RSS 1495 → 1008 MiB; decode speed unchanged | experiment 5 |
| Pause-split streaming in the helper | key-up 360 → ~21 ms with pauses; no-pause clips unchanged | "Experiment: streaming end to end" |
| Helper process for the model | unloaded RSS 647 MiB → under 60 MB after UNLOAD | "Experiment: helper process" |

Rejected or dropped:

| Idea | Result | Where |
|---|---|---|
| CoreML execution provider | encoder about 2× slower than CPU, 7 s cold load; MLProgram fails on the int8 encoder | experiment 2 |
| Optimized-model cache | cold load worse (3.3 s save, 2.7–3.1 s reload) | experiment 6 |
| Graph optimization level 1 or 2 | no warm memory gain; level 1 slower to load | experiment 5 |
| Prepacking off | −400 MB warm but −25% decode speed | experiment 5 |
| Memory-pattern off | no effect | experiment 5 |
| Overlapping-window frame splicing | unsound: the mel frontend and full attention shift every frame with window length | streaming experiment |
| `malloc_zone_pressure_relief` after each reply | 0 MB change; the retained memory is live | "Memory: helper steady state" |
| Decoder-joint prepacking off | 2 MB saved; decode noise | "Memory: helper steady state" |

Open levers, from `PERF.md`:

- A smaller or re-quantized encoder export. This is the only route to under 1 GB warm with the current model.
- Chunked encoding during the hold. Pause-split streaming covers most of this. The rest is an
  encoder-level change for long bursts without pauses.
- The v2 int8 model (English only) is smaller. It has not been measured on the reference machine.

## Rules for changes

- Measure before and after on the reference machine, with the same fixtures. Record both in `PERF.md`.
- A change to the decode path must keep WER and determinism (the bench and the ignored tests).
- Memory targets are checked by measurement, not by the bench: the bench reports RSS but does not gate it.
