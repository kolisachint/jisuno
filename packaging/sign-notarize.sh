#!/usr/bin/env bash
# Sign Jisuno.app with Developer ID (hardened runtime), notarize, staple, and zip.
#
# Usage: packaging/sign-notarize.sh [--app PATH] [--zip PATH] [--dry-run]
#   --app      Jisuno.app from bundle.sh (default <repo>/target/packaging/Jisuno.app)
#   --zip      release zip (default <app dir>/Jisuno-<version>-universal.zip)
#   --dry-run  print every command, run none, skip env checks
#
# Required env:
#   DEVELOPER_ID_APPLICATION   signing identity, e.g. "Developer ID Application: Name (TEAMID)"
# Notarization env, one of:
#   NOTARY_KEYCHAIN_PROFILE    profile from `xcrun notarytool store-credentials` (preferred)
#   APPLE_ID, APPLE_TEAM_ID, APPLE_APP_PASSWORD   app-specific password
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ENTITLEMENTS="$SCRIPT_DIR/jisuno.entitlements"

usage() {
  sed -n '2,13p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

die() {
  echo "error: $*" >&2
  exit 1
}

APP="$REPO_ROOT/target/packaging/Jisuno.app"
ZIP=""
DRY_RUN=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --app)
      APP="${2:-}"
      shift 2
      ;;
    --zip)
      ZIP="${2:-}"
      shift 2
      ;;
    --dry-run)
      DRY_RUN=true
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      usage >&2
      die "unknown argument: $1"
      ;;
  esac
done

# Run a command, or only print it under --dry-run.
run() {
  printf '+'
  printf ' %q' "$@"
  printf '\n'
  if ! $DRY_RUN; then
    "$@"
  fi
}

[[ -d "$APP" ]] || die "app bundle not found: $APP (run bundle.sh first)"
[[ -f "$ENTITLEMENTS" ]] || die "entitlements missing: $ENTITLEMENTS"

BIN="$APP/Contents/MacOS/jisuno"
PLIST="$APP/Contents/Info.plist"
if [[ -x "$BIN" ]]; then
  :
elif $DRY_RUN; then
  echo "note: $BIN not present (dry run, fake bundle)" >&2
else
  die "binary missing or not executable: $BIN"
fi

VERSION="unknown"
if [[ -f "$PLIST" ]]; then
  VERSION="$(plutil -extract CFBundleShortVersionString raw -o - "$PLIST")"
elif ! $DRY_RUN; then
  die "Info.plist missing: $PLIST"
fi
if [[ -z "$ZIP" ]]; then
  ZIP="$(dirname "$APP")/Jisuno-$VERSION-universal.zip"
fi

if $DRY_RUN; then
  echo "dry run: nothing will be signed, notarized, stapled or written"
  SIGN_ID="${DEVELOPER_ID_APPLICATION:-DEVELOPER_ID_APPLICATION}"
else
  [[ -n "${DEVELOPER_ID_APPLICATION:-}" ]] ||
    die "DEVELOPER_ID_APPLICATION is not set (e.g. 'Developer ID Application: Name (TEAMID)')"
  SIGN_ID="$DEVELOPER_ID_APPLICATION"

  if [[ -z "${NOTARY_KEYCHAIN_PROFILE:-}" ]]; then
    missing=()
    for v in APPLE_ID APPLE_TEAM_ID APPLE_APP_PASSWORD; do
      [[ -n "${!v:-}" ]] || missing+=("$v")
    done
    if ((${#missing[@]} > 0)); then
      die "notarization env missing: ${missing[*]} (or set NOTARY_KEYCHAIN_PROFILE)"
    fi
  fi

  identities="$(security find-identity -v -p codesigning)"
  grep -Fq "$SIGN_ID" <<<"$identities" ||
    die "signing identity not in keychain: $SIGN_ID"
fi

# Credentials for notarytool. The password is never echoed: notary_show is the
# loggable form.
if [[ -n "${NOTARY_KEYCHAIN_PROFILE:-}" ]]; then
  notary_auth=(--keychain-profile "$NOTARY_KEYCHAIN_PROFILE")
  notary_show=(--keychain-profile "$NOTARY_KEYCHAIN_PROFILE")
elif $DRY_RUN; then
  notary_auth=(--apple-id "${APPLE_ID:-APPLE_ID}" --team-id "${APPLE_TEAM_ID:-APPLE_TEAM_ID}"
    --password "${APPLE_APP_PASSWORD:-APPLE_APP_PASSWORD}")
  notary_show=(--apple-id "${APPLE_ID:-APPLE_ID}" --team-id "${APPLE_TEAM_ID:-APPLE_TEAM_ID}"
    --password '***')
else
  notary_auth=(--apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD")
  notary_show=(--apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password '***')
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
SUBMIT_ZIP="$WORK/Jisuno-notarize.zip"

echo "==> sign nested code"
if [[ -d "$APP/Contents/Frameworks" ]]; then
  while IFS= read -r -d '' lib; do
    run codesign --force --options runtime --timestamp --sign "$SIGN_ID" "$lib"
  done < <(find "$APP/Contents/Frameworks" -name '*.dylib' -print0)
fi
# The bundled benchmark helper (Settings -> Performance) is a nested executable.
BENCH="$APP/Contents/MacOS/jisuno-bench"
if [[ -f "$BENCH" ]]; then
  run codesign --force --options runtime --timestamp --sign "$SIGN_ID" "$BENCH"
elif ! $DRY_RUN; then
  die "jisuno-bench missing from the bundle: $BENCH (run bundle.sh first)"
fi

echo "==> sign app"
run codesign --force --options runtime --timestamp \
  --entitlements "$ENTITLEMENTS" --sign "$SIGN_ID" "$APP"

echo "==> verify signature"
run codesign --verify --deep --strict --verbose=2 "$APP"
# Gatekeeper rejects an un-notarized build here; that is expected at this stage.
run spctl -a -vv -t exec "$APP" ||
  echo "note: spctl rejection before notarization is expected" >&2

echo "==> submit for notarization"
run ditto -c -k --keepParent "$APP" "$SUBMIT_ZIP"
echo "+ xcrun notarytool submit $SUBMIT_ZIP --wait ${notary_show[*]}"
if ! $DRY_RUN; then
  # notarytool can exit 0 with a rejected status, so check the text too.
  out="$(xcrun notarytool submit "$SUBMIT_ZIP" --wait "${notary_auth[@]}" 2>&1)" || {
    printf '%s\n' "$out" >&2
    die "notarytool submit failed"
  }
  printf '%s\n' "$out"
  grep -q 'status: Accepted' <<<"$out" ||
    die "notarization not accepted; get the log with: xcrun notarytool log <submission-id> (same credentials)"
fi

echo "==> staple"
run xcrun stapler staple "$APP"
run xcrun stapler validate "$APP"

echo "==> zip release"
run ditto -c -k --keepParent "$APP" "$ZIP"
run spctl -a -vv -t exec "$APP"
run shasum -a 256 "$ZIP"

echo "==> done: $ZIP"
