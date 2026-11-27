# CI / Release workflows

The GitHub Actions workflows live in `.github/workflows/`: `ci.yml`,
`release.yml`, and `mirror-models.yml`. There is no manual activation step.
Design rules for what runs where: `docs/design/jisuno.md` §3 and §6.1.

Runner rule: anything that can be pure goes in `jisuno-logic` so it is tested
on Linux. macOS jobs run only what needs macOS.

## `ci.yml`

Triggers: push to `main`, pull requests, `workflow_dispatch`. Concurrency is
per workflow and ref; a new run cancels the older one in progress.

| Job | Runner | Steps |
|--|--|--|
| `core` | ubuntu-latest | installs `libasound2-dev`; `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace` |
| `macos` | macos-14 (Apple silicon) | `cargo clippy --workspace --all-targets -- -D warnings`; `cargo clippy -p jisuno --features app --all-targets -- -D warnings`; `cargo test --workspace`; `cargo test -p jisuno --features app,dev` (debug hooks); `cargo build --release -p jisuno --features app`; `packaging/bundle.sh --version 0.0.0 --arch arm64 --out dist`; uploads the unsigned app zip as `Jisuno-app-unsigned-arm64` (7 days) |
| `bench` | macos-14 | only on `workflow_dispatch` and on push to `main`. Restores the model cache (`jisuno-models-<model id>-v1`, `JISUNO_MODELS_DIR=$GITHUB_WORKSPACE/models`) and downloads the pinned model on a miss. Generates two `say` fixtures (converted to 16 kHz mono WAV with `afconvert`), runs `cargo run --release -p jisuno-bench -- --json --model parakeet-tdt-0.6b-v3-int8 --fixtures-dir <fixtures>`, uploads `jisuno-bench` (JSON, 7 days). Never gated: no `--gate`. |

`--version 0.0.0` is used for CI bundles because `bundle.sh` accepts only
`X.Y.Z`.

Not yet in CI: the ASR fixture job (design §3, "ASR fixtures", ubuntu).

## `release.yml`

Triggered when a PR is merged into `main` and carries a `cargo:patch`,
`cargo:minor` or `cargo:major` label. Only `main` is pushed to; every later job
checks out the tag `v<version>` so the artifacts match the released commit.
Runs are queued in the `release` concurrency group and never cancelled.

1. **bump-and-tag** — resolves the version (rules below), bumps `Cargo.toml`
   and the internal dependency pins, runs `cargo update --workspace`, commits
   `release: v<version>` to `main`, and creates an annotated `v*` tag. The commit
   and the tag go out in one atomic push. Nothing is pushed on a re-run that
   reuses a tag.
2. **publish** — publishes `jisuno-logic`, `jisuno-audio`, `jisuno-asr`,
   `jisuno-platform`, `jisuno-ui`, then `jisuno` to crates.io in dependency
   order (`jisuno`'s optional `app` feature depends on platform and ui, and
   cargo requires optional deps to be on the registry). A version already on
   the index is skipped, so a partial run can be retried. Needs
   `CRATES_IO_TOKEN`. `jisuno-bench` has `publish = false`.
   crates.io rate-limits brand-new crates (a burst, then about one per 10
   minutes). On a 429, "rate limit" or "too many" error the step sleeps 11
   minutes and retries, up to 4 times per crate, then fails. The job has
   `timeout-minutes: 90`.
3. **create-release** — creates the GitHub release `v<version>` with
   auto-generated notes (idempotent). Runs in parallel with `publish`.
4. **macos-app** — macos-14. Builds the universal `Jisuno.app` with
   `packaging/bundle.sh --arch universal`, then:
   - if all signing secrets are set: imports the Developer ID certificate into a
     temporary keychain, runs `packaging/sign-notarize.sh` (sign, notarize,
     staple, zip), and removes the keychain;
   - otherwise: warns and zips the app unsigned.

   Uploads `Jisuno-<version>-universal.zip` and its `.sha256` to the release.
   Outputs `signed` (`true`/`false`) for the cask job.
5. **cli** — builds `jisuno` without the `app` feature (`cargo build --release
   --locked -p jisuno --target <t>`) for `aarch64-apple-darwin`,
   `x86_64-apple-darwin` (cross-built on macos-14) and
   `x86_64-unknown-linux-gnu` (installs ALSA). Uploads
   `jisuno-<version>-<target>.tar.gz` and its `.sha256`.
6. **cask** — downloads the release zip, computes its sha256, renders
   `packaging/homebrew/jisuno.rb` (`__VERSION__`, `__SHA256__`) and checks it
   with `ruby -c`. Uploads `jisuno.rb` to the release only when the app was
   signed and notarized (`macos-app` output `signed` is `true`). Otherwise it
   warns and skips the upload. Copying the cask to the Homebrew tap is manual
   and out of scope.
7. **checksums** — aggregates every `*.sha256` asset into `SHA256SUMS`.

### Version rules

- Base: the highest `v*` tag reachable from `main` (`git tag --merged`, sorted
  by version). The label bumps it: `patch`, `minor` or `major`.
- No `v*` tag yet (first release): the `[workspace.package] version` in
  `Cargo.toml` is released unchanged. If the manifest already has that version,
  no bump commit is made; the tag `v<version>` points at the merge commit. The
  label is still required. With the manifest at `0.1.0`, the first labelled PR
  merged releases `v0.1.0`.
- Never edit `version` in a feature PR, and never tag by hand.

### Re-runs

- Re-run a failed run from the Actions UI (`Re-run failed jobs`, or
  `Re-run all jobs`). Every job is idempotent: the publish step skips versions
  already on crates.io, `gh release create` skips an existing release, and
  uploads use `--clobber`.
- `bump-and-tag` reuses the release when a `v*` tag points at this PR's merge
  commit, or at a commit whose parent is that merge commit (the release commit
  made on top of it). It then sets the `version` and `tag` outputs, pushes
  nothing, and the later jobs run against that tag.
- Caveat: if other commits landed on `main` after the merge, the release commit
  is no longer directly on top of it, the reuse check misses it, and a re-run
  bumps again. Check `git tag --list 'v*'` and `git log --oneline` before
  re-running in that case.
- To ship the signed app and cask after adding the signing secrets, re-run all
  jobs of the same run; the signing check runs again.

### Labels

The workflow only fires for these three labels. They must exist in the repo
(`/pr` creates them when missing). To create them by hand:

```bash
gh label create cargo:patch --color 0E8A16 --description "Release: patch bump on merge"
gh label create cargo:minor --color FBCA04 --description "Release: minor bump on merge"
gh label create cargo:major --color B60205 --description "Release: major bump on merge"
```

An error that the label already exists means it is there; ignore it.

### Push permission on `main`

`bump-and-tag` pushes the release commit and tag to `main`. The token is
`secrets.RELEASE_TOKEN` when that secret is set, else `GITHUB_TOKEN`.

- No branch protection on `main`, or protection that lets `github-actions[bot]`
  push: `GITHUB_TOKEN` is enough.
- `main` is protected against that bot: set `RELEASE_TOKEN`, a fine-grained PAT
  (or GitHub App token) with Contents read/write on this repo, owned by a user
  who can bypass the protection. Commits pushed with `RELEASE_TOKEN` also trigger
  `ci.yml` on `main`; `GITHUB_TOKEN` pushes do not.

### Secrets

Names only; values are never in the repo.

| Secret | Used by | Purpose |
|--|--|--|
| `CRATES_IO_TOKEN` | publish | crates.io API token |
| `RELEASE_TOKEN` | bump-and-tag | optional. Pushes the release commit and tag to `main` when `GITHUB_TOKEN` is not allowed to (see above) |
| `MACOS_CERT_P12_BASE64` | macos-app | base64 of the Developer ID Application `.p12` |
| `MACOS_CERT_PASSWORD` | macos-app | password of that `.p12` |
| `DEVELOPER_ID_APPLICATION` | macos-app | signing identity name, e.g. `Developer ID Application: Name (TEAMID)` |
| `APPLE_ID` | macos-app | Apple ID for notarization |
| `APPLE_TEAM_ID` | macos-app | Team ID for notarization |
| `APPLE_APP_PASSWORD` | macos-app | app-specific password for notarization |
| `GITHUB_TOKEN` | all | provided by Actions; release and asset writes. Pushes the bump commit only if `main` allows it |

The signing secrets are all-or-nothing: if any one is missing the app is
published unsigned with a warning, and the cask is not uploaded.
`NOTARY_KEYCHAIN_PROFILE` is not used in CI.

## `mirror-models.yml`

Manual (`workflow_dispatch`). Downloads the Parakeet v3 and v2 int8 model files
from HuggingFace and uploads them to this repo's `models-v1` release, as the
fallback source for `jisuno setup`. Uses only `GITHUB_TOKEN`. Not synced with
upstream; re-run by hand if upstream changes. `ci-templates/mirror-models.yml`
is a copy of the same file; keep both identical.

## PR-based release flow

1. Run `/pr patch` (or `minor` / `major`). The command creates the
   `cargo:<bump>` label if missing and opens a PR with it
   (`.agents/commands/pr.md`).
2. Merge the PR into `main`. `release.yml` runs.
3. The workflow takes the version (highest `v*` tag plus the label; the
   manifest version on the first release), bumps, tags, publishes the crates,
   creates the release, and attaches the app zip, CLI archives, the cask (signed
   builds only) and `SHA256SUMS`.

Do not edit `version` in a feature PR, and do not tag by hand.

## Why a single workflow?

Tags pushed by the `GITHUB_TOKEN` do not trigger other workflows (a GitHub
Actions safety measure), so a split bump → tag-push → release design would
require a manual tag re-push every time. Combining version bumping and releasing
into one workflow eliminates that.

## Follow-ups (not done here)

- Homebrew tap push (`brew audit --cask --strict` and install check) is manual.
