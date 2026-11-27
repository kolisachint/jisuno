# Release

Releases are cut from a labeled pull request. Nobody edits the version by hand and nobody tags by hand.
This page is the overview. The workflow, secrets and packaging details are in
[`ci/README.md`](../../../ci/README.md) (workflows) and [`packaging/README.md`](../../../packaging/README.md)
(bundle, signing, cask). Those two files are the references; check them before a release.

## Flow

1. Open a release PR with `/pr patch`, `/pr minor` or `/pr major` (default `patch`). The command is defined in
   [`.agents/commands/pr.md`](../../../.agents/commands/pr.md). It labels the PR `cargo:patch`, `cargo:minor` or
   `cargo:major`.
2. Merge the PR. `.github/workflows/release.yml` runs on the merge.
3. The workflow computes the next version from `Cargo.toml` and the label, bumps it, commits
   `release: v<version>` to `main`, and creates the annotated tag `v<version>`.
4. It publishes the crates to crates.io, creates the GitHub release, and attaches:
   - `Jisuno-<version>-universal.zip` (the macOS app) and its `.sha256`;
   - CLI archives for macOS (Apple silicon and Intel) and Linux x86_64, with `.sha256` files;
   - the rendered Homebrew cask `jisuno.rb`;
   - `SHA256SUMS`.

Versions follow `AGENTS.md`: `patch` for fixes and additions, `minor` for API or behavior changes, `major`
for large breaking changes.

## Things to know

- **Signing is optional in the workflow.** If the signing secrets are missing, the app is zipped unsigned and
  the job warns. An unsigned build is blocked by Gatekeeper. The user guide says so in
  [Install](../../user/src/install.md).
- **Crates.** `jisuno-logic`, `jisuno-audio`, `jisuno-asr`, `jisuno-platform`, `jisuno-ui` and `jisuno` are
  published in dependency order. A version already on the index is skipped. `jisuno-bench` is not published.
- **Homebrew.** Copying `jisuno.rb` to the tap is manual. Check the cask with `brew audit --cask --strict` and
  an install from the zip before it goes out. There is no tap yet.
- **Secrets.** The names are in `ci/README.md`. Values are never in the repo.
- **First release.** The first release uses the version already in `Cargo.toml` (`0.1.0`), not a bump. The
  release workflow is being changed in a separate change, so check `release.yml` before you rely on this.
- **Version source.** Later releases take the workspace version in `Cargo.toml` and bump it from the label.

## Local dry run

Packaging runs on a Mac with the Rust Apple targets installed:

```sh
packaging/bundle.sh --version 0.1.0 --arch universal --out target/packaging
packaging/sign-notarize.sh --app target/packaging/Jisuno.app --dry-run
```

`bundle.sh` builds release binaries and assembles the bundle, unsigned. `sign-notarize.sh` needs the signing
environment (see [Building](building.md#permissions-and-signing)); `--dry-run` prints its commands only.
