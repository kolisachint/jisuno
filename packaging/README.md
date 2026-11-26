# packaging

macOS app bundle, signing, notarization and the Homebrew cask for Jisuno.

## Files

| File | What it is |
|--|--|
| `Info.plist` | Bundle metadata. `__VERSION__` is filled by `bundle.sh`. `LSUIElement` true (no Dock icon). Microphone usage string. |
| `jisuno.entitlements` | Hardened-runtime entitlements (audio input only). Each omitted entitlement is explained in the file. |
| `bundle.sh` | `cargo build --release -p jisuno --features app` per target, `lipo` for universal, assembles `Jisuno.app`, bundles `libonnxruntime*.dylib` if present. |
| `sign-notarize.sh` | Developer ID signing, verify, notarize, staple, zip. `--dry-run` prints the commands only. |
| `homebrew/jisuno.rb` | Cask template. `__VERSION__` and `__SHA256__` are filled per release. |
| `AppIcon.icns` | Optional. Copied into the bundle if present (not yet created). |

## Secrets and environment

Names only; values never go in the repo. The CI secrets are listed in `ci/README.md`.

- `DEVELOPER_ID_APPLICATION`: signing identity name, e.g. `Developer ID Application: Name (TEAMID)`. The certificate must be in the keychain.
- Notarization, one of:
  - `NOTARY_KEYCHAIN_PROFILE`: profile from `xcrun notarytool store-credentials` (preferred locally; the password stays out of the process list).
  - `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD`: app-specific password (what CI uses).
- CI only: `MACOS_CERT_P12_BASE64` and `MACOS_CERT_PASSWORD` (the Developer ID `.p12`, base64 and password). `release.yml` imports them into a temporary keychain.

## Release flow (CI)

`.github/workflows/release.yml` runs after a `cargo:<bump>` PR is merged. Its `macos-app` job runs the steps below, then the `cask` job renders the cask. Nothing is done by hand.

1. `packaging/bundle.sh --version X.Y.Z --arch universal --out dist`: builds both targets and assembles `dist/Jisuno.app`, unsigned.
2. If the signing secrets are set: `packaging/sign-notarize.sh --app dist/Jisuno.app --zip dist/Jisuno-X.Y.Z-universal.zip`: signs, notarizes, staples and zips. If not, the job zips the app unsigned and warns.
3. Uploads `Jisuno-X.Y.Z-universal.zip` and its `.sha256` to the GitHub release `vX.Y.Z`. The version comes from the bump step; do not bump `Cargo.toml` by hand.
4. `cask` job: sha256 of the uploaded zip, rendered into `homebrew/jisuno.rb` (`__VERSION__`, `__SHA256__`), uploaded to the release as `jisuno.rb`. The template is not edited in place.
5. Copy `jisuno.rb` to the Homebrew tap. This is manual and out of scope for CI. Do not copy an unsigned build (the job warns). Then check the cask: `brew audit --cask --strict jisuno` and `brew install --cask jisuno`, launch, and confirm the four permissions.

Local run (same steps, on a Mac with the Rust targets): run the two scripts above by hand with `--out target/packaging` (the script default).

## Permissions

TCC grants (Microphone, Accessibility, Input Monitoring) are tied to the app's signing identity (its designated requirement) and bundle id. A dev build signed ad hoc, or with a different identity, gets a new identity, so macOS will re-prompt after each rebuild. Test permission flows with a build signed by the same Developer ID as the release.
