#!/usr/bin/env bash
# Build the Intent 2.0 .deb for the host architecture.
#
# Run INSIDE the Ubuntu VM / build host (not on macOS):
#     packaging/build-deb.sh [--output dist/intent_2.0.0_arm64.deb] [--no-build]
#
# Environment:
#   CARGO_TARGET_DIR      cargo output dir (default: <repo>/target)
#   INTENT_FIREFOX_XPI    Mozilla-signed XPI → release build (installs the Firefox policy);
#                         otherwise capture/integrations/firefox-extension/dist/*.xpi is
#                         bundled as a dev build (no policy) if present.
#   INTENT_BUILD_APP=1    also build and package the Tauri app from app/ (see HOOK below).
set -euo pipefail
umask 022

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
VERSION=2.0.0
ARCH=$(dpkg --print-architecture)
OUTPUT="$ROOT/dist/intent_${VERSION}_${ARCH}.deb"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
BUILD=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --output) OUTPUT=$2; shift ;;
    --no-build) BUILD=0 ;;
    -h|--help) sed -n '2,13p' "$0"; exit 0 ;;
    *) echo "Unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done

warn() { echo "build-deb: warning: $*" >&2; }

if [[ $BUILD -eq 1 ]]; then
  (cd "$ROOT" && cargo build --release --locked -p intentd -p intentctl)
fi
for bin in intentd intentctl; do
  [[ -x "$TARGET_DIR/release/$bin" ]] || { echo "missing $TARGET_DIR/release/$bin" >&2; exit 2; }
done

STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
chmod 0755 "$STAGE"
SHARE="$STAGE/usr/share/intent"
mkdir -p "$STAGE/DEBIAN" "$STAGE/usr/bin" "$STAGE/usr/lib/systemd/user" "$SHARE/shell" \
  "$SHARE/integrations" "$SHARE/firefox" "$STAGE/usr/share/gnome-shell/extensions" \
  "$STAGE/usr/share/doc/intent"

install -m 0755 "$TARGET_DIR/release/intentd" "$TARGET_DIR/release/intentctl" "$STAGE/usr/bin/"
install -m 0644 "$ROOT/packaging/systemd/intentd.service" "$STAGE/usr/lib/systemd/user/intentd.service"
install -m 0644 "$ROOT/shell/intent.bash" "$ROOT/shell/intent.zsh" "$SHARE/shell/"
cp -r "$ROOT/gnome-extension/intent-focus@intent.local" "$STAGE/usr/share/gnome-shell/extensions/"
find "$STAGE/usr/share/gnome-shell/extensions" -type f -exec chmod 0644 {} +
install -m 0644 "$ROOT/packaging/debian/firefox-policies.json" "$SHARE/firefox/policies.json"
printf 'Intent %s\nhttps://github.com/Pranav5255/Intent\n' "$VERSION" > "$STAGE/usr/share/doc/intent/README"

# Companions: bundle what was built, warn about the rest.
BUILD_MODE=dev
if [[ -n "${INTENT_FIREFOX_XPI:-}" ]]; then
  [[ -f "$INTENT_FIREFOX_XPI" ]] || { echo "INTENT_FIREFOX_XPI not found: $INTENT_FIREFOX_XPI" >&2; exit 2; }
  install -m 0644 "$INTENT_FIREFOX_XPI" "$SHARE/integrations/intent-firefox.xpi"
  BUILD_MODE=release
else
  xpi=$(ls "$ROOT"/capture/integrations/firefox-extension/dist/*.xpi 2>/dev/null | head -n1 || true)
  if [[ -n "$xpi" ]]; then
    install -m 0644 "$xpi" "$SHARE/integrations/intent-firefox.xpi"
    warn "bundling unsigned Firefox XPI ($xpi); dev build, Firefox policy will not be installed"
  else
    warn "no Firefox XPI under capture/integrations/firefox-extension/dist; companion skipped"
  fi
fi
vsix="$ROOT/capture/integrations/vscode-extension/dist/intent-vscode.vsix"
if [[ -f "$vsix" ]]; then
  install -m 0644 "$vsix" "$SHARE/integrations/intent-vscode.vsix"
else
  warn "no VS Code VSIX at $vsix; companion skipped"
fi
echo "$BUILD_MODE" > "$SHARE/BUILD_MODE"

DEPENDS="libc6 (>= 2.35), libgcc-s1"

# ---------------------------------------------------------------------------
# HOOK(app): the Tauri desktop app (being built in app/ on another branch).
# When INTENT_BUILD_APP=1 and app/src-tauri exists, build it and install the
# binary as /usr/lib/intent/intent-app behind the /usr/bin/intent launcher,
# plus its desktop entry and icon, and add its runtime libraries to Depends.
# ---------------------------------------------------------------------------
if [[ "${INTENT_BUILD_APP:-0}" == "1" ]]; then
  if [[ -d "$ROOT/app/src-tauri" ]]; then
    (cd "$ROOT/app" && npm ci && npm run tauri build -- --no-bundle)
    app_bin=$(find "$ROOT/app/src-tauri/target/release" -maxdepth 1 -type f -perm -u+x -name 'intent*' | head -n1)
    [[ -n "$app_bin" ]] || { echo "Tauri build produced no binary" >&2; exit 2; }
    install -D -m 0755 "$app_bin" "$STAGE/usr/lib/intent/intent-app"
    install -m 0755 "$ROOT/packaging/debian/intent-launcher" "$STAGE/usr/bin/intent"
    [[ -f "$ROOT/app/packaging/intent.desktop" ]] &&
      install -D -m 0644 "$ROOT/app/packaging/intent.desktop" "$STAGE/usr/share/applications/intent.desktop"
    [[ -f "$ROOT/app/packaging/intent.svg" ]] &&
      install -D -m 0644 "$ROOT/app/packaging/intent.svg" "$STAGE/usr/share/icons/hicolor/scalable/apps/intent.svg"
    DEPENDS="$DEPENDS, libgtk-3-0t64 | libgtk-3-0, libwebkit2gtk-4.1-0, libayatana-appindicator3-1"
  else
    warn "INTENT_BUILD_APP=1 but app/src-tauri is missing; app skipped"
  fi
fi

INSTALLED_SIZE=$(du -sk --exclude=DEBIAN "$STAGE" | cut -f1)
sed -e "s/@VERSION@/$VERSION/" -e "s/@ARCHITECTURE@/$ARCH/" -e "s/@INSTALLED_SIZE@/$INSTALLED_SIZE/" \
  -e "s/@DEPENDS@/$DEPENDS/" "$ROOT/packaging/debian/control.in" > "$STAGE/DEBIAN/control"
for script in postinst prerm postrm; do
  install -m 0755 "$ROOT/packaging/debian/maintainer-scripts/$script" "$STAGE/DEBIAN/$script"
done

mkdir -p "$(dirname "$OUTPUT")"
dpkg-deb --root-owner-group -Zxz --build "$STAGE" "$OUTPUT" >/dev/null
echo "Built $OUTPUT ($(du -h "$OUTPUT" | cut -f1), installed size ${INSTALLED_SIZE} KiB, $BUILD_MODE companions)"
