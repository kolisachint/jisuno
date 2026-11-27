# Introduction

This is the developer guide for Jisuno: how the code is laid out, how to build, test and release it,
and where the measured numbers and design decisions are recorded.

- User documentation (install, usage, config files): [`docs/user/`](../../user/src/SUMMARY.md).
- Design record (signed-off decisions and specs): [`docs/design/`](../../design/README.md). Code follows
  these documents, not the reverse. Decisions are listed in [Decisions](decisions.md).
- Measured performance: [Performance](performance.md), with the full method in
  [`crates/jisuno-asr/PERF.md`](../../../crates/jisuno-asr/PERF.md).
- Working rules for agents and humans: [`AGENTS.md`](../../../AGENTS.md) and [Contributing](contributing.md).

Jisuno is a Cargo virtual workspace with `crates/*`. The macOS app and the `jisuno` command are one
binary (`crates/jisuno`). Code that can be pure lives in `jisuno-logic` and is tested on Linux.
