# ASR perf sweep (2026-11-26)

Reference machine: the owner’s Mac (Apple M1, 8 GB). Model:
`parakeet-tdt-0.6b-v3-int8`. Harness: `cargo run --release -p jisuno-bench
-- --model parakeet-v3 --fixtures-dir /tmp/fixtures --runs 5 --warmup 1`
(medians below; machine was shared with other agents, so every shortlisted
config was measured back-to-back under load < 8 and min/max were tight).

Fixtures (`say`/`afconvert`, 16 kHz mono, with `.txt` refs): 2.7 s
(“quick brown fox”), 6.3 s (meeting), 8.3 s (names), 11.4 s (numbers),
12.9 s (order address). Baseline hypotheses (byte-exact bar for every
experiment — WER must not regress and every fixture must decode 10/10
identically): fox sentence exact; “Please schedule a meeting with Dr.
Alvarez for Thursday at 330 …”; order-4793/$1256.18/42 Baker Street…;
“Shivan O’Connor … Dr. Wynne … Yayo Kusama … Wolf Gang Amadeus”;
“Call extension 5550198 … 314159 … $7,200”. Baseline WER mean 0.336
(normalization-heavy: “Dr.” vs “Doctor”, “Siobhan”→“Shivan”).

Targets (docs/design/jisuno.md §6, §6.1): 5 s final decode ≤ 100 ms warm;
key-up→text ≤ 150 ms; cold load ≤ 1.5 s; warm mem < 1 GB; unloaded mem
< 60 MB.

## Experiment 1 — profile (kept: `JISUNO_PROFILE=1` per-stage timings)

5 s clip, final config: mel 8 ms, **encoder 340 ms, decoder 13 ms**
(63 frames, 37 joint steps). Decoder ≈ 4 % of decode ⇒ the joint loop is
not the lever; encoder is. (Decoder inputs are already zero-copy
`TensorRef`s; per-step logits copies are ~1 K floats — negligible.)

## Results table (median of 5; RSS in MiB)

| experiment | cold load | 2 s | 5 s | 15 s | key-up 5 s | RSS warm | RSS unloaded | WER | 10x determ. |
|--|--|--|--|--|--|--|--|--|--|
| A baseline (intra 1) | 1313 ms | 248 ms | 573 ms | 1692 ms | 572 ms | 1532 | 1281 | 0.336 | yes |
| B intra 4 + determ. | 1287 ms | 197 ms | 385 ms | 1058 ms | 397 ms | 1598 | 1204 | 0.336 | yes |
| C intra 4, no determ. | 1205 ms | 186 ms | 419 ms | 1070 ms | 388 ms | 1661 | 983 | 0.336 | yes |
| D **intra 2 + determ.** | 1238 ms | 154 ms | **351 ms** | **1014 ms** | **369 ms** | 1495 | 988 | 0.336 | yes |
| E intra 3 + determ. | 1258 ms | 175 ms | 369 ms | 1052 ms | 389 ms | 1532 | 1205 | 0.336 | — |
| F intra 2, no determ. | 1178 ms | 164 ms | 384 ms | 1042 ms | 381 ms | 1657 | 1044 | 0.336 | yes |
| G D + no arena | 1120 ms | 171 ms | 366 ms | 1041 ms | 364 ms | **1008** | **649** | 0.336 | yes |
| H D + mem_pattern off | 1253 ms | 155 ms | 379 ms | 1073 ms | 373 ms | 1578 | 1141 | 0.336 | — |
| I D + opt Level2 | 1278 ms | 153 ms | 361 ms | 1063 ms | 355 ms | 1549 | 1134 | 0.336 | — |
| J D + opt Level1 | 1417 ms | 177 ms | 388 ms | 1062 ms | 386 ms | 1577 | 976 | 0.336 | — |
| K D + prepack off | 794 ms | 224 ms | 448 ms | 1169 ms | 455 ms | 1106 | 1116 | 0.336 | — |
| K2 D + prepack off + no arena | 817 ms | 208 ms | 410 ms | 1118 ms | 412 ms | 1109 | 1353 | 0.336 | — |
| L D + no arena + opt-model cache (save) | 3337 ms | 165 ms | 366 ms | 1056 ms | 367 ms | 719* | 362* | 0.336 | — |
| M/M2 same, cache warm | 3067/2715 ms | — | 374/393 ms | 1052/1043 ms | 376/365 ms | 1044/1204 | 570/632 | 0.336 | — |
| CoreML enc-only (CPU+NE, cache) | 6943 ms | 964 ms | 1244 ms | 4785 ms | 2045 ms | 815 | 1058 | 0.336 | steps differ† |
| CoreML all sessions | 4324 ms | 782 ms | 903 ms | 3803 ms | 1678 ms | 1176 | 831 | 0.336 | — |
| **FINAL (baked-in = G)** | **1156 ms** | **171 ms** | **378 ms** | **1015 ms** | **360 ms** | **1008** | **647** | **0.336** | **yes** |

\* L’s RSS low outlier did not reproduce (M/M2 back at ~1.1 GB) — allocator/page-cache noise, not an effect.
† CoreML encoder took a different greedy path on the looped-noise clip (42 vs 37 steps) but identical WER text on all real fixtures.

## Per-experiment verdicts

1. **Profile** — encoder is ~92 % of warm decode. Kept `JISUNO_PROFILE`
   instrumentation only.
2. **Execution providers** — CoreML EP (ort `coreml` feature, tried
   locally, then removed): encoder-only ≈ 2x slower than CPU at equal
   WER and 7 s cold load even with a compiled-model cache; all-sessions
   worse; MLProgram format hard-errors building the int8 encoder plan
   (`Failed to create MLModel … error code: -14`). The int8 graph falls
   back to CPU per-node plus partitioning overhead. **Dropped.**
3. **Threading** — intra 2 + `with_deterministic_compute(true)` wins
   (5 s: 351 vs 573 ms, −39 %; 15 s: 1014 vs 1692 ms, −40 %).
   Deterministic flag costs nothing measurable (D vs F within noise) and
   keeps the 10x guarantee the old single-thread comment wanted; without
   it all 50 probe decodes also passed on this ORT/M1 combo, but the flag
   stays as belt and braces. inter-op untouched (single sequential run —
   nothing to parallelize). **Kept.**
4. **Decoder loop** — 13 ms of a 368 ms decode. Not pursued beyond the
   existing zero-copy inputs.
5. **Memory** — `with_memory_pattern(false)`: no effect. Lower opt
   levels: same RSS, Level1 slower cold load. Prepacking off: −400 MB
   warm but −25 % decode speed and −400 ms cold-load win — worse trade
   than the arena knob. **Disabling the CPU arena: warm 1495→1008 MiB
   (−33 %), unloaded 988→649, decode speed unchanged. Kept.**
   `malloc_zone_pressure_relief` after unload is called by the bench
   (best-effort; the product needs the same call on its unload path).
   `footprint` on the unloaded process: 233 MB dirty (126 MB Malloc
   Large + 103 MB Malloc Small retained by libmalloc/ORT globals), the
   rest of RSS is reclaimable file pages — RSS overstates the leak, but
   even dirty is far from the 60 MB target (see below).
6. **Cold load** — `with_optimized_model_path` under the cache dir:
   first load 3.3 s (optimize+save), cache-warm loads 2.7–3.1 s vs
   1.1 s without — ORT still re-runs optimization; no benefit.
   **Dropped.** `commit_from_file` already takes the file path (no extra
   copy to avoid); `commit_from_memory` is for `.ort` packages.

## Before → after (baked-in = D + no-arena)

| metric | before (A) | after | target | pass |
|--|--|--|--|--|
| cold load | 1313 ms | 1156 ms | ≤ 1.5 s | ✅ |
| warm decode 5 s | 573 ms | 378 ms (−34 %) | ≤ 100 ms | ❌ |
| key-up→text 5 s | 572 ms | 360 ms (−37 %) | ≤ 150 ms | ❌ |
| warm RSS | 1532 MiB | 1008 MiB (−34 %) | < 1024 MiB | ✅ (marginal) |
| unloaded RSS / dirty | 1281 / — | 647 / ~233 | < 60 MB | ❌ |
| WER (5 fixtures) | 0.336 | 0.336 | no worse | ✅ |
| determinism (10x × 5 fixtures) | yes | yes | yes | ✅ |

Targets now passing: cold load, warm memory (just under 1 GB — one
regression or a bigger model revision blows it), WER/determinism.
Still failing: 5 s warm decode (378 vs 100 ms) and key-up→text (360 vs
150 ms) — both are the same bottleneck: **full-clip encoder forward
pass at key-up**.

## Recommended next lever

Chunked/incremental encoding during the hold (design §6 already names
it): run the encoder over 0.5–1 s windows while the key is down and, at
key-up, encode only the tail plus a small overlap for the mel frontend’s
receptive field / FastConformer subsampling lookahead. The measured
encoder cost is ~70 ms per second of audio (351 ms / 5 s), so a ≤1 s
tail keeps the key-up encode under ~100 ms budget and the joint loop
(~2.4 ms/s) is negligible. The TDT decoder itself is stateless across
frames except the emitted-token predictor state, so a chunk boundary
only needs the predictor state carried over — no architecture change.
Second lever if that isn’t enough: a smaller encoder export (the v2
int8 model) or static int8 quantization with fewer prepacked constants
to buy warm-RSS headroom.

## Experiment: pause-split streaming (2026-11-27, kept)

Overlapping-window frame splicing was tried first and abandoned as
unsound. Probing showed the mel frontend normalizes per utterance
(a windowed mel of identical audio hits cosine −0.21 against the
full-clip mel) and the full-attention encoder shifts every frame with
window length (frame 0 already diverges with identical left context).
Carried predictor state then cascades one divergent window into total
silence: with `(C, L, R) = (1.0, 2.0, 0.5)` short clips decoded empty,
and the best grid point `(0.5, 2.0, 0.25)` still lost +0.050 WER mean
with catastrophic single-window failures. No window length is safe, so
frame reuse across lengths was dropped entirely.

Kept design: cut pause-bounded segments with a sample-count energy gate
(hysteresis 0.02/0.01 RMS, 20 ms frames), decode each on a background
worker thread with a fresh predictor state, `finish` decodes only the
tail. Cut at the middle of a >= pause run following >= min-speech of
speech; force-cut at the lowest-energy frame of the last 2 s after
8 s without a pause; join segments with single spaces plus a documented
sentence-case heuristic. No predictor state crosses segments, so one
bad segment cannot poison the rest. `StreamingTranscriber` and the
`--streaming` bench mode implement this.

Fixtures added (local, `/tmp/fixtures`): `pause45` (32.9 s, 7 distinct
`say` sentences joined with 0.4–0.7 s digital silence) and
`continuous40` (29.6 s, same sentences back-to-back, no gaps —
exercises the forced cut). Existing `loop39`/`loop46` cover repeated
continuous content.

### Config grid (WER mean over 9 pairs; full-clip mean 0.3875)

| (pause, min-speech, max-seg) | stream WER | delta vs full |
|--|--|--|
| (0.30, 1.00, 8.0) default-then | 0.3943 | +0.0068 |
| **(0.20, 1.00, 8.0) kept default** | **0.3857** | **−0.0018** |
| (0.20, 0.50, 8.0) | 0.3857 | −0.0018 |
| (0.30, 1.50, 8.0) | 0.3943 | +0.0068 |
| (0.30, 1.00, 5.0) | 0.4209 | +0.0334 |
| (0.15, 0.50, 8.0) | 0.3994 | +0.0119 |

Chosen `(0.2, 1.0, 8.0)`: 0.5 s min-speech ties it but 1.0 s keeps
segments longer (short windows decode worse); 0.15 s pauses cut
mid-word; 5 s forced cuts hurt continuous audio. Per-fixture (kept
config, runs = 5): continuous40 0.289, long 0.579, loop39 0.500,
loop46 0.647, med 0.150, names 0.348, numbers 0.647, pause45 0.311,
say 0.000 vs full 0.311/0.579/0.526/0.647/0.150/0.304/0.647/0.322/0.000.
Remaining deltas are small boundary instabilities (`Shivan/Shivon`,
`Yayo/Yayoy`, digit phrasing). Deterministic across 10 runs
(`streaming_is_deterministic_across_runs`).

### Latency (kept config, runs = 5, load ~5.5 on 8 cores)

- push of 100 ms audio: median ~8 us, max 0.1 ms (timer floor in the
  bench table). Keeps up easily; no ONNX work happens in `push`.
- `finish()` on pauseless clips (whole tail, no overlap in bench): 5 s
  median 560 ms (min 489), 15 s median 1715 ms (min 1531).
- `finish()` production-like (queue drained during speech): ~0 ms when
  key-up lands in trailing silence (tail skipped by the energy gate:
  pause45-full, continuous40-full, med); 321 ms for a 4.6 s mid-utterance
  tail, i.e. ~70 ms per tail second. So ≤ 120 ms holds iff the tail since
  the last pause is ≤ ~1.7 s — the normal case for dictation rhythm
  (pauses every few seconds), not for a long final burst.
- Idempotent repeat `finish()`: ~20 us.

Guard streaming WER ≤ full + 0.01: pass (−0.001 on the kept
config). The 150 ms key-up target now depends on tail length rather
than total length: with pauses it is typically one short tail decode
(tens of ms); without pauses it degrades to a full-clip decode.

Bench: `cargo run --release -p jisuno-bench -- --fixtures-dir
/tmp/fixtures --runs 5 --warmup 1 --streaming --stream-config
"0.2,1.0,8.0"`. Integration test:
`cargo test --release -p jisuno-asr --test streaming -- --ignored`
(skips without model/fixtures).

## Experiment: helper process (kept: `worker/` in jisuno-asr)

In-process `drop` of the sessions leaves ~650 MiB RSS (FINAL row above:
647 MiB, ~233 MiB dirty in libmalloc/ORT globals) against the < 60 MB
unloaded target — allocator memory the process cannot give back. Fix:
run the model in a helper child (`AsrWorker` client +
`run_worker_stdio` server in `crates/jisuno-asr/src/worker/`, framed
stdin/stdout protocol, stderr for logs). UNLOAD makes the child exit 0
and the OS reclaims everything. The app will spawn it later from a
hidden `jisuno asr-worker` subcommand; lifecycle tests drive
`examples/asr_worker.rs` (`--fake` = no-model backend).

Measured release, same M1/8 GB machine, v3 model, 5 s fixture clip
(`cargo test --release -p jisuno-asr --test worker -- --ignored
--nocapture`, plus a `ps`-sampling driver for RSS):

| metric | in-process | helper child | target | pass |
|--|--|--|--|--|
| RSS with model unloaded | 647 MiB (dirty ~233) | parent 11 MiB, fresh child 4 MiB, child gone after UNLOAD | < 60 MB | ✅ |
| `finish()` 5 s (one PUSH + FINISH) | 390–392 ms | 370–375 ms (delta −15/−22 ms, run-to-run noise) | overhead < 5 ms | ✅ |
| cold start | load 1287–1615 ms | spawn 1 ms + LOAD ~1260 ms (wall 1267–1530 ms) | ≈ in-process | ✅ |
| UNLOAD latency | n/a (drop) | 40–57 ms, exit 0 | prompt | ✅ |
| text parity | — | byte-identical on the 5 s clip (PARTIAL == FINISH too) | identical | ✅ |

Child warm RSS: 1079 MiB loaded, 1116 MiB after one decode — same
model cost, just in a disposable process. Parent (driver) RSS is 11 MiB
before and after UNLOAD: the app process never maps the weights.
`finish()` shows no systematic overhead (framing + two pipe hops for
320 KB of f32 is sub-millisecond; the −20 ms delta is cache/scheduling
noise across the two runs). No ORT/threading change: the child links
the same sessions with the same baked-in config.

Follow-up for the app wiring (not this change): spawn on first use,
`load` on setup/install, `PARTIAL_REQ` on `--partial-ms`, `unload` on
quit; treat `WorkerCrashed` as "re-spawn and reload".

## Experiment: streaming end to end (2026-11-27, kept)

The helper child now serves streaming utterances on one shared session
(`worker/server.rs`: BEGIN/PUSH/PARTIAL_REQ/FINISH/CANCEL drive a single
`ParakeetStreamingSession`; TRANSCRIBE runs full-clip on the same worker
sessions — one model in memory). PARTIAL returns joined finished segments
(no re-decode). The app feeds debug WAVs in 100 ms wall-clock chunks
while the hold is down, drains the mic through `StreamingResampler`
(chunked output == one-shot within 1e-5), and draws live partials in the
HUD (`ListeningUi.partial` → `listening_state` → `refresh_hud_now` on
arrival, only when `settings.partials`).

Measured live with the release app, isolated `JISUNO_CONFIG_DIR`
(model `at_login`, `partials: true`, `hud: off`), `say` WAVs at 16 kHz
mono, `--debug-hold-ms` = duration + 300 ms, holds started warm. The
screen was locked (Secure Input on, loginwindow holding it), so holds
used a temporary local bypass — reverted before commit (`grep`
`JISUNO_E2E` clean) — TextEdit could not come frontmost, and the HUD
could not render: numbers below are from the `key-up -> text` / `partial`
log lines only.

| clip | length | pauses | key-up→text (3 runs) | median | finish ms | target ≤ 150 ms |
|--|--|--|--|--|--|--|
| `say` meeting | 7.0 s | 1 × 0.6 s | 22, 21, 22 | **22** | 0, 0, 0 | ✅ |
| `say` mixed | 17.4 s | 3 × 0.5 s | 21, 21, 50 | **21** | 9, 16, 44 | ✅ |
| `say` numbers, no pause | 5.8 s | none | 162 | 162 | 150 | ❌ (marginal) |

Text parity: all three clips decode to the expected words (19/32/6
words; `Dr. Alvarez`, `$1256.18`, `order 4793` intact). Partials logged
every 400 ms mid-hold, growing as segments finished
(`Please schedule…` → full first sentence → …); the no-pause clip
logged no partials — correct, nothing had finished. Key-up in trailing
silence skips the tail encode (finish 0 ms); the 50 ms w19 run caught a
mid-utterance tail (finish 44 ms ≈ 0.6 s of tail).

Before → after: with-pauses key-up→text 360 ms (full-clip at key-up) →
~21 ms (tail only), −94 %. Without pauses it degrades to a full-clip
tail decode (162 ms at 5.8 s) — the documented limit, not a regression.

## Memory: helper steady state (2026-11-27, no change kept)

Goal: helper warm steady-state `phys_footprint` under 1 GB (1024 MB)
with decode no more than 10 % slower. **Not met.** Baseline is 1120 MB
after decode, about 100 MB over. No candidate below moved it.

Harness: `asr_worker` release build (`--features dev`), driven by a
framed-protocol script: LOAD `parakeet-tdt-0.6b-v3-int8`, three
TRANSCRIBE calls on a 5 s clip, `footprint <pid>` after LOAD, after
the third decode, and 2 s later. Five repeats each, medians. Decode ms
is the median of the three TRANSCRIBE replies. The bench's `warm decode
5 s` is not a clean A/B on this machine: baseline ran 450 ms and
candidate 2 ran 385 ms, back-to-back under load.

| config | load MB | post-decode MB | idle-after-decode MB | decode 5 s ms |
|--|--|--|--|--|
| baseline (d3cc82e) | 1084 | 1120 | 1120 | 348 |
| 1. `malloc_zone_pressure_relief` after FINISH/TRANSCRIBE/CANCEL | 1084 | 1120 | 1120 | 351 |
| 2. decoder_joint prepacking off | 1082 | 1120 | 1120 | 374 (noisy) |

Candidate 1 (libmalloc pressure relief after each reply) changed
nothing: 0 MB at every point. The retained memory is live, not free
allocator pages. Dropped without a commit.

Candidate 2 (`session.disable_prepacking` on decoder_joint only, 18 MB
file) saved 2 MB. Decode 374 vs 348 ms is inside the noise seen here.
Dropped without a commit.

Dead ends from earlier runs (not re-measured here):

- Encoder prepacking off: about −35 MB but +12–28 % decode. Rejected.
  Even with that, the post-decode footprint stays around 1085 MB, over
  the target.
- ORT opt level 1 or 2 vs 3, `with_memory_pattern(false)`: no change in
  warm RSS.
- Env allocator settings and mmap-backed loading: no help.
- Arena off (already shipped): −33 % RSS, no decode change. Its
  remaining cost is the live weights.

Where the 1084 MB at load sits: the encoder is 652 MB on disk (int8,
inline initializers). Earlier footprint breakdown at load: about 760 MB Malloc Large and
about 350 MB Malloc Small. Getting under 1 GB needs a change at the
model or session level, not allocator tuning. Options:

- A smaller or re-quantized encoder export (the item in the earlier
  "Recommended next lever" note).
- An encoder session change, which is out of scope for this experiment.
  The builder is shared with the decoder, so it needs a separate decision.
