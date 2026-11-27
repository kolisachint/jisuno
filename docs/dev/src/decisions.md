# Decisions

The design record is in [`docs/design/`](../../design/README.md). `jisuno.md` §10 is the decisions log. Its
rounds win over older text in any doc. The voice-command and keybinding changelogs are in
[`voice-control.md`](../../design/voice-control.md) (changelog) and [`keybindings.md`](../../design/keybindings.md)
(§11). The UI decisions are in [`app-ui.md`](../../design/app-ui.md) §11 and §12. All of them were signed off
on 2026-11-26, round 6 for the UI.

## Index

| Round | Where | Decisions (one line each) |
|---|---|---|
| 1 | [jisuno.md §10](../../design/jisuno.md#10-decisions-log) | Hotkey only, no wake word. `voice-control-config.json` owns commands. Two hold keys, rebindable. Speed is measured. Model loads on demand and unloads when idle. Mic opens on key-down. GPUI. |
| 2 | [jisuno.md §10](../../design/jisuno.md#10-decisions-log) | macOS only. Fixed commands, no accessibility traversal (superseded by round 10). Action set in §5.2. Agents use `jisuno control`. Confirm dangerous commands. `{slot}` parameters. Warm key separate from hold keys. Idle unload 10 min. |
| 3 | [jisuno.md §10](../../design/jisuno.md#10-decisions-log) | Hold-key conflicts are handled by rebinding. Platform code for the menu bar and HUD is accepted. Config in `~/.config/jisuno/`. The `voicetools` crate is left as is. Reference machine is an M1 with 8 GB. Fresh git history. |
| 4 | [jisuno.md §10](../../design/jisuno.md#10-decisions-log) | `serve` and its line protocol stay. Whisper is dropped. Parakeet v3 int8 is the default, v2 optional. First run writes all four config files with `$schema`. Paste is the default insert, type is the fallback. Focus lost keeps the text in Recent. Secure input pauses the keys. First hold key wins. 2 min cap. Mic picker, Bluetooth warning. Fixed slot lists. Per-app `when`. No spoken punctuation or voice editing. Recent in memory only. Signed app, cask and crates. Manual updates. |
| 5 | [jisuno.md §10](../../design/jisuno.md#10-decisions-log) | Jisuno and `voicetools` are separate products with no runtime relation. Jisuno has its own model folder and mirror. |
| 6 | [jisuno.md §10](../../design/jisuno.md#10-decisions-log), [app-ui.md §12](../../design/app-ui.md#12-decisions-signed-off-in-round-6) | UI signed off. macOS system fonts only. Targets: cold load ≤ 1.5 s, warm memory < 1 GB (superseded by round 9), unloaded < 60 MB. `login_item` is the key name. Partials live in `settings.json`. `theme.json` has a schema and defaults. |
| 7 | [voice-control.md changelog](../../design/voice-control.md#changelog) | One tokenizer for voice text. Duplicate-phrase and slot checks (22–25). Malformed declarations are errors. Matcher: per-word distance 0.34 with the same first letter, free slots at most 10 words, long utterances rejected. Runner fill-failure copy. |
| 8 | [voice-control.md changelog](../../design/voice-control.md#changelog) | `ax.*` uses a bounded search (depth ≤ 4, ≤ 300 elements). `window.tile center` is 60% of the visible frame. `shell.run` runs `/bin/zsh -lc` with stdin closed and a 60 s timeout. |
| 9 | [jisuno.md §10](../../design/jisuno.md#10-decisions-log) | The model runs in a helper process (`jisuno asr-worker`), so unloading frees its memory. Warm memory target < 1.2 GB. Pause-split streaming with live partials. Debug hooks only in `dev` builds. Logs carry no speech text unless `JISUNO_LOG_TEXT=1`. |
| 10 | [jisuno.md §10](../../design/jisuno.md#10-decisions-log) | `ax.*` uses direct attributes, then the bounded search (supersedes "never a scan"). `language` removed from `settings.json`. `login_item` uses `SMAppService.mainApp`. Settings → Performance runs the bundled `jisuno-bench`. There is no `jisuno bench` subcommand. |

Keybindings changelog (`keybindings.md` §11): v2 made Right ⌘ the command key and ⌃⌥W the warm key. Round 4
added first-key-wins, the object form of `keybindings.json`, Return in `confirming`, the 150 ms minimum and the
2 min cap.

## Superseded

| Earlier text | Superseded by |
|---|---|
| Warm memory < 1 GB (round 6, item 3) | Round 9, item 2: < 1.2 GB |
| "No accessibility-tree traversal" (round 2, item 2; `jisuno.md` §5.2) | Round 10, item 1: bounded search |
| `language` setting (round 4 and `jisuno.md` §4) | Round 10, item 2: removed |
| Partials re-decode in `serve.rs` as the speed lever (`jisuno.md` §6) | Round 9, item 3: pause-split streaming in the helper. `serve` still re-decodes. |
| `hud.show_partials` in `theme.json` (`app-ui.md`) | Round 6, item 7: `partials` in `settings.json` |

## Known differences between design and code

These are open. Fix the code, or record a decision in the design record. Do not change the design silently.

| # | Design says | Code does | Where |
|---|---|---|---|
| 1 | Settings → Performance reports WER on LibriSpeech test-clean and an owner set (`jisuno.md` §6.1) | The app runs `jisuno-bench` with no fixtures. The base audio is synthetic and WER is `n/a`. LibriSpeech is not in the repo. | `app/windows.rs` (`run_bench`), `jisuno-bench` |
| 2 | Bench reports idle CPU (`jisuno.md` §6.1) | Not measured. | `jisuno-bench/src/lib.rs` |
| 3 | ASR fixture job on ubuntu in CI (`jisuno.md` §3) | Not in `ci.yml`. `ci/README.md` lists it as a follow-up. | `.github/workflows/ci.yml` |
| 4 | Memory is a §6.1 target | Reported by the bench but not checked; `--gate` covers only decode and key-up. | `jisuno-bench/src/lib.rs` |
| 5 | `keybindings.md` §1: "No code exists yet" | The code exists and `defaults()` in `jisuno-logic` is the source. The status line is stale. | `docs/design/keybindings.md` |

The design docs are not edited here. Add a round to `jisuno.md` §10 when a decision changes.
