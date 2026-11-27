# Contributing

The full rules are in [`AGENTS.md`](../../../AGENTS.md). This page is the short version for people. Agents
follow `AGENTS.md` as written.

## Before you change code

- Read the design record for the area (`docs/design/`). Code follows the design, not the other way round.
  Decisions are in [Decisions](decisions.md).
- Read the whole file before a wide change or an edit to a file you have not read.
- Put pure logic in `jisuno-logic` so it is tested on Linux.
- Errors use `anyhow` with `?`. No `unwrap()` or `expect()` outside tests.
- Keep the ONNX tensor names as the constants at the top of
  `crates/jisuno-asr/src/transcribe/parakeet.rs`.
- Backward compatibility is not kept unless the user asks for it. Ask before removing a feature that looks
  intentional.
- Match the surrounding style: import order, naming, comments.

## Gates

After a code change (not a doc-only change), run and fix:

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Add or change a test, run it, and iterate until it passes. On macOS, also run
`cargo test -p jisuno --features app,dev` when the app or the platform crates change. Details:
[Testing](testing.md).

## Commits

- Commit only when the user asks.
- Commit only the files you changed in this session. Check `git status` first.
- Stage paths by name: `git add <file> ...`. Never `git add -A` or `git add .`. Other work may be in the
  same tree.
- Subjects are short and typed, for example `fix(hud): ...`, `docs(user): ...`, `feat(asr): ...`. Add
  `fixes #<n>` or `closes #<n>` in the body when an issue applies.
- No emojis in commits, issues, PR comments or code.

Forbidden, because they can destroy other work:

| Command | Why not |
|---|---|
| `git reset --hard`, `git checkout .`, `git clean -fd` | discard uncommitted work |
| `git stash` | hides other work |
| `git add -A`, `git add .` | stages other people's files |
| `git commit --no-verify` | skips the hooks |
| force push over shared history | rewrites what others have |

If a rebase conflicts in a file you did not change, stop and ask. Resolve conflicts in your own files only.

## Pull requests and releases

- Open a release PR with `/pr <patch|minor|major>` (see [Release](release.md)).
- Never edit `version = "…"` in `Cargo.toml` in a feature PR. The release workflow owns the version.
- Never tag by hand.

## Keep docs in step

When behavior changes, update the docs in the same change:

| Doc | What it covers |
|---|---|
| [`README.md`](../../../README.md) | the front page: features, install, usage, status |
| [`docs/user/src/`](../../user/src/SUMMARY.md) | user guide (mdBook) |
| [`docs/dev/src/`](../../dev/src/SUMMARY.md) | this developer guide |
| [`docs/user/src/tui.md`](../../user/src/tui.md) and `tui/voice-transcribe.ts` | TUI wiring reference |
| [`crates/jisuno-asr/PERF.md`](../../../crates/jisuno-asr/PERF.md) | measured numbers; keep the README and [Performance](performance.md) in sync |
| [`ci/README.md`](../../../ci/README.md), [`packaging/README.md`](../../../packaging/README.md) | CI and packaging |

Do not edit `docs/design/` to match code. When a decision changes, record it as a new round in
`docs/design/jisuno.md` §10.

## Working in parallel

Several agents may work in one tree. Check `git status` before and after each step, and do not change files another
agent owns. A parallel writer gets its own worktree and branch.
