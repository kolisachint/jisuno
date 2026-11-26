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
