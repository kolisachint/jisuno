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

Names only; values never go in the repo.

- `DEVELOPER_ID_APPLICATION`: signing identity name, e.g. `Developer ID Application: Name (TEAMID)`. The certificate must be in the keychain.
- Notarization, one of:
  - `NOTARY_KEYCHAIN_PROFILE`: profile from `xcrun notarytool store-credentials` (preferred; the password stays out of the process list).
  - `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD`: app-specific password.

## Release flow

1. Build and assemble, unsigned: `packaging/bundle.sh --version X.Y.Z --arch universal --out target/packaging`. Needs the `aarch64-apple-darwin` and `x86_64-apple-darwin` rustup targets.
2. Sign, notarize, staple and zip: `packaging/sign-notarize.sh --app target/packaging/Jisuno.app` (env as above). It prints the zip path and its sha256.
3. Create a GitHub release `vX.Y.Z` and attach `Jisuno-X.Y.Z-universal.zip`. The tag must match the version that `release.yml` computes; do not bump `Cargo.toml` by hand.
4. Put the sha256 and version into `homebrew/jisuno.rb` (copy it to the tap, not edited in place here).
5. Check the cask in the tap: `brew audit --cask --strict jisuno` and `brew install --cask jisuno`, then launch and confirm the four permissions.

## Permissions

TCC grants (Microphone, Accessibility, Input Monitoring) are tied to the app's signing identity (its designated requirement) and bundle id. A dev build signed ad hoc, or with a different identity, gets a new identity, so macOS will re-prompt after each rebuild. Test permission flows with a build signed by the same Developer ID as the release.
