#!/usr/bin/env bash
# Build the Intent 2.0 .deb for the host architecture.
#
# Run INSIDE the Ubuntu VM / build host (not on macOS):
#     packaging/build-deb.sh [--output dist/intent_2.0.0_arm64.deb] [--no-build]
#
# Environment:
#   CARGO_TARGET_DIR      cargo output dir (default: <repo>/target)
#   INTENT_FIREFOX_XPI    Mozilla-signed XPI → release build (installs the Firefox policy);
#                         otherwise integrations/firefox-extension/dist/*.xpi is
#                         bundled as a dev build (no policy) if present.
#   INTENT_BUILD_APP=1    also build and package the Tauri app from app/ (needs Node 22 and
#                         the webkit2gtk-4.1 / appindicator -dev packages; see "App" below).
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
    -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
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
  xpi=$(ls "$ROOT"/integrations/firefox-extension/dist/*.xpi 2>/dev/null | head -n1 || true)
  if [[ -n "$xpi" ]]; then
    install -m 0644 "$xpi" "$SHARE/integrations/intent-firefox.xpi"
    warn "bundling unsigned Firefox XPI ($xpi); dev build, Firefox policy will not be installed"
  else
    warn "no Firefox XPI under integrations/firefox-extension/dist; companion skipped"
  fi
fi
vsix="$ROOT/integrations/vscode-extension/dist/intent-vscode.vsix"
if [[ -f "$vsix" ]]; then
  install -m 0644 "$vsix" "$SHARE/integrations/intent-vscode.vsix"
else
  warn "no VS Code VSIX at $vsix; companion skipped"
fi
echo "$BUILD_MODE" > "$SHARE/BUILD_MODE"

DEPENDS="libc6 (>= 2.35), libgcc-s1"

# ---------------------------------------------------------------------------
# App: the Tauri desktop app from app/ (INTENT_BUILD_APP=1).
# Binary in /usr/lib/intent/intent-app behind the /usr/bin/intent launcher, a
# desktop entry and hicolor icons. No system-wide XDG autostart: it would start
# the app (and, through the launcher, the capture service) for users who never
# opted in. "Launch at login" in the app's settings writes the per-user entry.
# Runtime libraries come from dpkg-shlibdeps; the tray library is dlopen()ed by
# libappindicator-sys, so it is added explicitly.
# ---------------------------------------------------------------------------
app_depends() {
  local tmp out
  tmp=$(mktemp -d)
  mkdir -p "$tmp/debian" && touch "$tmp/debian/control"
  out=$(cd "$tmp" && dpkg-shlibdeps -O "${@/#/-e}" 2>/dev/null | sed -n 's/^shlibs:Depends=//p') || true
  rm -rf "$tmp"
  if [[ -z "$out" ]]; then
    warn "dpkg-shlibdeps unavailable or failed; using the static app dependency list"
    out="libc6 (>= 2.39), libgcc-s1, libglib2.0-0t64 | libglib2.0-0, libgtk-3-0t64 | libgtk-3-0, libwebkit2gtk-4.1-0, libjavascriptcoregtk-4.1-0, libsoup-3.0-0, libcairo2, libgdk-pixbuf-2.0-0, libpango-1.0-0, libxdo3"
  fi
  echo "$out, libayatana-appindicator3-1 | libappindicator3-1"
}

if [[ "${INTENT_BUILD_APP:-0}" == "1" ]]; then
  if [[ -d "$ROOT/app/src-tauri" ]]; then
    if [[ $BUILD -eq 1 ]]; then
      # tauri runs `npm run build` (beforeBuildCommand), enables the
      # custom-protocol feature and honours CARGO_TARGET_DIR.
      (cd "$ROOT/app" && npm ci --no-audit --no-fund && npx tauri build --ci --no-bundle -- --locked)
    fi
    app_bin="$TARGET_DIR/release/intent-app"
    [[ -x "$app_bin" ]] || { echo "missing $app_bin (Tauri build)" >&2; exit 2; }
    install -D -m 0755 "$app_bin" "$STAGE/usr/lib/intent/intent-app"
    install -m 0755 "$ROOT/packaging/debian/intent-launcher" "$STAGE/usr/bin/intent"
    install -D -m 0644 "$ROOT/packaging/debian/intent.desktop" \
      "$STAGE/usr/share/applications/io.github.pranav5255.Intent.desktop"
    icons="$ROOT/app/src-tauri/icons"
    for size in 32 64 128; do
      install -D -m 0644 "$icons/${size}x${size}.png" "$STAGE/usr/share/icons/hicolor/${size}x${size}/apps/intent.png"
    done
    install -D -m 0644 "$icons/128x128@2x.png" "$STAGE/usr/share/icons/hicolor/256x256/apps/intent.png"
    install -D -m 0644 "$icons/icon.png" "$STAGE/usr/share/icons/hicolor/512x512/apps/intent.png"
    DEPENDS=$(app_depends "$STAGE/usr/lib/intent/intent-app" "$STAGE/usr/bin/intentd" "$STAGE/usr/bin/intentctl")
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
