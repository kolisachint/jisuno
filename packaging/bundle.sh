#!/usr/bin/env bash
# Build the release binary and assemble Jisuno.app (unsigned).
# Signing and notarization are separate: run sign-notarize.sh on the result.
#
# Usage: packaging/bundle.sh --version X.Y.Z [--arch arm64|x86_64|universal] [--out DIR]
#   --arch  default universal (needs both rustup targets installed)
#   --out   default <repo>/target/packaging
#
# Output: DIR/Jisuno.app/Contents/{MacOS/{jisuno,jisuno-bench},Info.plist,Resources/,Frameworks/}
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}"
APP_NAME="Jisuno.app"
BIN_NAME="jisuno"
BENCH_NAME="jisuno-bench"
RPATH_FLAG="@executable_path/../Frameworks"

usage() {
  sed -n '2,8p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

die() {
  echo "error: $*" >&2
  exit 1
}

# Point the binary at bundled dylibs: add the rpath, and rewrite absolute
# references to libonnxruntime (compile-time linking only). Run on a thin slice.
rewrite_bin() {
  local bin="$1" rpaths refs ref
  rpaths="$(otool -l "$bin")"
  if ! grep -Fq "$RPATH_FLAG" <<<"$rpaths"; then
    install_name_tool -add_rpath "$RPATH_FLAG" "$bin"
  fi
  refs="$(otool -L "$bin" | awk 'NR > 1 { print $1 }')"
  while IFS= read -r ref; do
    case "$ref" in
      /*/libonnxruntime*.dylib)
        install_name_tool -change "$ref" "@rpath/$(basename "$ref")" "$bin"
        ;;
    esac
  done <<<"$refs"
}

VERSION=""
ARCH="universal"
OUT="$REPO_ROOT/target/packaging"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version)
      VERSION="${2:-}"
      shift 2
      ;;
    --arch)
      ARCH="${2:-}"
      shift 2
      ;;
    --out)
      OUT="${2:-}"
      shift 2
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

[[ -n "$VERSION" ]] || die "--version X.Y.Z is required"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "--version must be X.Y.Z, got '$VERSION'"
[[ -n "$OUT" ]] || die "--out needs a directory"
[[ "$(uname -s)" == "Darwin" ]] || die "bundle.sh must run on macOS"

case "$ARCH" in
  arm64) TARGETS=(aarch64-apple-darwin) ;;
  x86_64) TARGETS=(x86_64-apple-darwin) ;;
  universal) TARGETS=(aarch64-apple-darwin x86_64-apple-darwin) ;;
  *) die "--arch must be arm64, x86_64 or universal, got '$ARCH'" ;;
esac

command -v cargo >/dev/null || die "cargo not found"
command -v lipo >/dev/null || die "lipo not found (install Xcode command line tools)"
command -v install_name_tool >/dev/null || die "install_name_tool not found (Xcode command line tools)"

if command -v rustup >/dev/null; then
  installed="$(rustup target list --installed)"
  for t in "${TARGETS[@]}"; do
    grep -Fxq "$t" <<<"$installed" || die "rustup target missing: rustup target add $t"
  done
fi

# Minimum OS for the whole bundle (LSMinimumSystemVersion in Info.plist).
export MACOSX_DEPLOYMENT_TARGET=13.0
# Header padding lets install_name_tool add the rpath and rewrite dylib paths
# in rewrite_bin; without it those rewrites fail with "larger updated load commands".
export RUSTFLAGS="${RUSTFLAGS:-} -C link-arg=-Wl,-headerpad_max_install_names"

for t in "${TARGETS[@]}"; do
  echo "==> cargo build --release --locked -p jisuno --features app --target $t"
  (cd "$REPO_ROOT" && cargo build --release --locked -p jisuno --features app --target "$t")
  echo "==> cargo build --release --locked -p jisuno-bench --target $t"
  (cd "$REPO_ROOT" && cargo build --release --locked -p jisuno-bench --target "$t")
done

APP="$OUT/$APP_NAME"
MACOS_DIR="$APP/Contents/MacOS"
RES_DIR="$APP/Contents/Resources"
FW_DIR="$APP/Contents/Frameworks"
BIN="$MACOS_DIR/$BIN_NAME"
BENCH="$MACOS_DIR/$BENCH_NAME"

echo "==> assembling $APP"
rm -rf "$APP"
mkdir -p "$MACOS_DIR" "$RES_DIR"

# Bundled ONNX Runtime, only if the built dir has one. ort links statically by
# default, so this is normally a no-op. Dylibs are taken from the first target dir
# and must exist in every target dir (lipo'd for universal).
DYLIBS=()
shopt -s nullglob
for f in "$TARGET_DIR/${TARGETS[0]}/release"/libonnxruntime*.dylib; do
  DYLIBS+=("$(basename "$f")")
done
shopt -u nullglob

# Per-target directories holding the binary (and dylibs) to lipo from. With dylibs,
# each thin slice is copied to a stage dir, edited and ad-hoc signed there, then
# lipo'd: editing or signing a fat file fails when a slice is unsigned.
SRC_DIRS=()
if ((${#DYLIBS[@]} > 0)); then
  STAGE="$(mktemp -d)"
  trap 'rm -rf "$STAGE"' EXIT
  for t in "${TARGETS[@]}"; do
    d="$STAGE/$t"
    mkdir -p "$d"
    cp "$TARGET_DIR/$t/release/$BIN_NAME" "$d/$BIN_NAME"
    cp "$TARGET_DIR/$t/release/$BENCH_NAME" "$d/$BENCH_NAME"
    for name in "${DYLIBS[@]}"; do
      src="$TARGET_DIR/$t/release/$name"
      [[ -f "$src" ]] || die "$name missing for $t (needed for universal)"
      cp -L "$src" "$d/$name"
      install_name_tool -id "@rpath/$name" "$d/$name"
      codesign --force --sign - "$d/$name" >/dev/null
    done
    rewrite_bin "$d/$BIN_NAME"
    codesign --force --sign - "$d/$BIN_NAME" >/dev/null
    rewrite_bin "$d/$BENCH_NAME"
    codesign --force --sign - "$d/$BENCH_NAME" >/dev/null
    SRC_DIRS+=("$d")
  done
else
  for t in "${TARGETS[@]}"; do
    SRC_DIRS+=("$TARGET_DIR/$t/release")
  done
fi

# One executable from every target dir: lipo'd for universal, copied otherwise.
combine_bin() {
  local name="$1" dest="$2" srcs=() d
  for d in "${SRC_DIRS[@]}"; do
    srcs+=("$d/$name")
  done
  if ((${#srcs[@]} > 1)); then
    lipo -create -output "$dest" "${srcs[@]}"
  else
    cp "${srcs[0]}" "$dest"
  fi
  chmod 755 "$dest"
}

combine_bin "$BIN_NAME" "$BIN"
echo "    binary: $(lipo -archs "$BIN")"
combine_bin "$BENCH_NAME" "$BENCH"
echo "    bench: $(lipo -archs "$BENCH")"

if ((${#DYLIBS[@]} > 0)); then
  mkdir -p "$FW_DIR"
  for name in "${DYLIBS[@]}"; do
    srcs=()
    for d in "${SRC_DIRS[@]}"; do
      srcs+=("$d/$name")
    done
    if ((${#srcs[@]} > 1)); then
      lipo -create -output "$FW_DIR/$name" "${srcs[@]}"
    else
      cp "${srcs[0]}" "$FW_DIR/$name"
    fi
    echo "    dylib: Frameworks/$name"
  done
fi

# Info.plist with the version placeholders filled in.
sed "s/__VERSION__/$VERSION/g" "$SCRIPT_DIR/Info.plist" >"$APP/Contents/Info.plist"
grep -q '__VERSION__' "$APP/Contents/Info.plist" && die "unsubstituted __VERSION__ in Info.plist"
plutil -lint "$APP/Contents/Info.plist" >/dev/null

ICON="$SCRIPT_DIR/AppIcon.icns"
if [[ -f "$ICON" ]]; then
  cp "$ICON" "$RES_DIR/AppIcon.icns"
else
  echo "warning: $ICON not found; the bundle has no app icon (generic icon will show)" >&2
fi

echo "==> built $APP (version $VERSION, $ARCH)"
