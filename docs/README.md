# Docs

Three documents, three audiences. Start with the one you need.

| Folder | Book | For | Start at |
|---|---|---|---|
| [`user/`](user/src/SUMMARY.md) | mdBook: user guide | People who use Jisuno: install, permissions, dictation, voice commands, settings, config files, agents, FAQ | [Introduction](user/src/introduction.md) |
| [`dev/`](dev/src/SUMMARY.md) | mdBook: developer guide | People who build, test, release or change the code: architecture, build, tests, performance, protocols, decisions | [Introduction](dev/src/introduction.md) |
| [`design/`](design/README.md) | Design record (Markdown) | Decisions and specs, signed off. The code follows these documents. | [README](design/README.md) |

Outside `docs/`:

| File | What it is |
|---|---|
| [`../README.md`](../README.md) | Front page: what Jisuno is, install, usage, status |
| [`../crates/jisuno-asr/PERF.md`](../crates/jisuno-asr/PERF.md) | Measured latency and memory, with method and rejected experiments |
| [`../AGENTS.md`](../AGENTS.md) | Working rules for agents and contributors |
| [`../ci/README.md`](../ci/README.md) | CI and release workflows |
| [`../packaging/README.md`](../packaging/README.md) | App bundle, signing, notarization, cask |
| [`../MODELS_NOTICE.md`](../MODELS_NOTICE.md) | Model sources and licenses |

## Build the books

Both books are mdBook projects. Install mdBook (`cargo install mdbook`), then run from `docs/`:

```sh
mdbook build user
mdbook build dev
mdbook serve dev        # local preview on http://localhost:3000
```

Output goes to `user/book/` and `dev/book/`, which are ignored by git.

## Keeping docs current

When behavior changes, update the user guide, the developer guide and the README in the same change. The design
record changes only by a new round in [`design/jisuno.md`](design/jisuno.md) §10. See
[Contributing](dev/src/contributing.md).
