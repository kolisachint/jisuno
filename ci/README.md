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
| `macos` | macos-14 (Apple silicon) | `cargo clippy --workspace --all-targets -- -D warnings`; `cargo clippy -p jisuno --features app --all-targets -- -D warnings`; `cargo test --workspace`; `cargo build --release -p jisuno --features app`; `packaging/bundle.sh --version 0.0.0 --arch arm64 --out dist`; uploads the unsigned app zip as `Jisuno-app-unsigned-arm64` (7 days) |
| `bench` | macos-14 | only on `workflow_dispatch` and on push to `main`. Restores the model cache (`jisuno-models-<model id>-v1`, `JISUNO_MODELS_DIR=$GITHUB_WORKSPACE/models`) and downloads the pinned model on a miss. Generates two `say` fixtures (converted to 16 kHz mono WAV with `afconvert`), runs `cargo run --release -p jisuno-bench -- --json --model parakeet-tdt-0.6b-v3-int8 --fixtures-dir <fixtures>`, uploads `jisuno-bench` (JSON, 7 days). Never gated: no `--gate`. |

`--version 0.0.0` is used for CI bundles because `bundle.sh` accepts only
`X.Y.Z`.

Not yet in CI: the ASR fixture job (design §3, "ASR fixtures", ubuntu).

## `release.yml`

Triggered when a PR with a `cargo:patch`, `cargo:minor`, or `cargo:major` label
is merged. Only `main` is pushed to; every later job checks out the tag
`v<version>` so the artifacts match the released commit.

1. **bump-and-tag** — reads the workspace version from `Cargo.toml`, bumps it by
   the label, updates the version pins on the `crates/*` path dependencies in
   the root `Cargo.toml`, runs `cargo update --workspace`, commits
   `release: v<version>` to `main`, pushes, and creates an annotated `v*` tag.
2. **publish** — publishes `jisuno-logic`, `jisuno-audio`, `jisuno-asr`, then
   `jisuno` to crates.io in dependency order. A version already on the index is
   skipped, so a partial run can be retried. Needs `CRATES_IO_TOKEN`.
   `jisuno-platform`, `jisuno-ui` and `jisuno-bench` are never published. They
   need `publish = false` in their `Cargo.toml` (not yet present; see below).
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
   `packaging/homebrew/jisuno.rb` (`__VERSION__`, `__SHA256__`), checks it with
   `ruby -c`, and uploads `jisuno.rb` to the release. Warns when the app is
   unsigned. Copying the cask to the Homebrew tap is manual and out of scope.
7. **checksums** — aggregates every `*.sha256` asset into `SHA256SUMS`.

### Secrets

Names only; values are never in the repo.

| Secret | Used by | Purpose |
|--|--|--|
| `CRATES_IO_TOKEN` | publish | crates.io API token |
| `MACOS_CERT_P12_BASE64` | macos-app | base64 of the Developer ID Application `.p12` |
| `MACOS_CERT_PASSWORD` | macos-app | password of that `.p12` |
| `DEVELOPER_ID_APPLICATION` | macos-app | signing identity name, e.g. `Developer ID Application: Name (TEAMID)` |
| `APPLE_ID` | macos-app | Apple ID for notarization |
| `APPLE_TEAM_ID` | macos-app | Team ID for notarization |
| `APPLE_APP_PASSWORD` | macos-app | app-specific password for notarization |
| `GITHUB_TOKEN` | all | provided by Actions; release and asset writes |

The signing secrets are all-or-nothing: if any one is missing the app is
published unsigned with a warning. `NOTARY_KEYCHAIN_PROFILE` is not used in CI.

## `mirror-models.yml`

Manual (`workflow_dispatch`). Downloads the Parakeet v3 and v2 int8 model files
from HuggingFace and uploads them to this repo's `models-v1` release, as the
fallback source for `jisuno setup`. Uses only `GITHUB_TOKEN`. Not synced with
upstream; re-run by hand if upstream changes. `ci-templates/mirror-models.yml`
is a copy of the same file; keep both identical.

## PR-based release flow

1. Run `/pr patch` (or `minor` / `major`). This opens a PR with the `cargo:<bump>`
   label (`.agents/commands/pr.md`).
2. Merge the PR. `release.yml` runs.
3. The workflow bumps the version, tags, publishes the crates, creates the
   release, and attaches the app zip, CLI archives, cask, and `SHA256SUMS`.

Do not edit `version` in a feature PR, and do not tag by hand.

## Why a single workflow?

Tags pushed by the `GITHUB_TOKEN` do not trigger other workflows (a GitHub
Actions safety measure), so a split bump → tag-push → release design would
require a manual tag re-push every time. Combining version bumping and releasing
into one workflow eliminates that.

## Follow-ups (not done here)

- `publish = false` is needed in `crates/jisuno-platform/Cargo.toml`,
  `crates/jisuno-ui/Cargo.toml`, and `crates/jisuno-bench/Cargo.toml`. The
  publish job never lists them, but `cargo publish` of `jisuno` fails if its
  `app` feature (once added) depends on an unpublished crate, even optionally.
  Keep the `app` deps out of `jisuno`'s published manifest, or make them
  publishable.
- `crates/jisuno-bench/Cargo.toml` pins `jisuno-asr` and `jisuno-audio` with
  `version = "0.1.0"`. A minor or major bump breaks that path dependency. Switch
  to `{ workspace = true }`.
- Homebrew tap push (`brew audit --cask --strict` and install check) is manual.
