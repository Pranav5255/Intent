#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
ENGINE_ROOT=$(cd "$ROOT/../engine" && pwd)
DESKTOP_ROOT=$(cd "$ROOT/../desktop/app" && pwd)
VERSION=1.0.0
ARCHITECTURE=$(dpkg --print-architecture)
PYTHON_VERSION=$(python3 -c 'import sys; print(f"{sys.version_info.major}.{sys.version_info.minor}")')
PYTHON_PACKAGE="python${PYTHON_VERSION}"
OUTPUT="$ROOT/dist/intent_${VERSION}_${ARCHITECTURE}.deb"
FIREFOX_XPI="${INTENT_FIREFOX_XPI:-}"
DESKTOP_ELECTRON="$DESKTOP_ROOT/node_modules/electron/dist"

usage() {
  cat <<'EOF'
Usage: packaging/build-deb.sh [--output path]

Builds the Intent .deb package. Requires INTENT_FIREFOX_XPI to point to a
Mozilla-signed Firefox XPI before packaging.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --output) OUTPUT=$2; shift ;;
    --help) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

if [[ -z "$FIREFOX_XPI" ]]; then
  echo "Build requires INTENT_FIREFOX_XPI=/path/to/signed.xpi" >&2
  exit 2
fi
[[ -f "$FIREFOX_XPI" ]] || { echo "Firefox XPI not found: $FIREFOX_XPI" >&2; exit 2; }
[[ -f "$ROOT/integrations/vscode-extension/dist/intent-vscode.vsix" ]] || {
  echo "Build the bundled VSIX before packaging." >&2
  exit 2
}
[[ -d "$ENGINE_ROOT/intent_engine" ]] || { echo "Engine source is unavailable: $ENGINE_ROOT" >&2; exit 2; }
[[ -x "$DESKTOP_ELECTRON/electron" ]] || {
  echo "Desktop Electron runtime is unavailable; run npm ci in desktop/app first." >&2
  exit 2
}
[[ -x "$DESKTOP_ROOT/node_modules/.bin/vite" ]] || {
  echo "Desktop build tooling is unavailable; run npm ci in desktop/app first." >&2
  exit 2
}
python3 -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 11) else 1)' || {
  echo "Engine packaging requires Python 3.11 or newer." >&2
  exit 2
}

npm --prefix "$DESKTOP_ROOT" run build

STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
PAYLOAD="$STAGE/opt/intent"
mkdir -p "$PAYLOAD" "$STAGE/DEBIAN" "$STAGE/usr/bin" "$STAGE/usr/lib/systemd/user" \
  "$STAGE/usr/share/applications" "$STAGE/usr/share/icons/hicolor/scalable/apps"

cp -a "$ROOT/event_server" "$ROOT/collectors" "$ROOT/shell" "$ROOT/integrations" \
  "$ROOT/scripts" "$ROOT/tools" "$ROOT/packaging" "$PAYLOAD/"
mkdir -p "$PAYLOAD/engine"
cp -a "$ENGINE_ROOT/intent_engine" "$PAYLOAD/engine/"
install -m 0644 "$ENGINE_ROOT/requirements.txt" "$ENGINE_ROOT/requirements-openai.txt" \
  "$ENGINE_ROOT/requirements-gemini.txt" "$ENGINE_ROOT/requirements-bedrock.txt" "$PAYLOAD/engine/"
find "$PAYLOAD" -type d \( -name tests -o -name __pycache__ \) -prune -exec rm -rf {} +
rm -f "$PAYLOAD/integrations/firefox-extension/dist/intent-firefox.xpi"
install -D -m 0644 "$FIREFOX_XPI" "$PAYLOAD/integrations/firefox-extension/dist/intent-firefox.xpi"
printf '%s\n' "release" > "$PAYLOAD/BUILD_MODE"

mkdir -p "$PAYLOAD/desktop/app" "$PAYLOAD/desktop/electron"
cp -a "$DESKTOP_ROOT/dist" "$DESKTOP_ROOT/electron" "$DESKTOP_ROOT/package.json" "$PAYLOAD/desktop/app/"
cp -a "$DESKTOP_ELECTRON/." "$PAYLOAD/desktop/electron/"
chmod 4755 "$PAYLOAD/desktop/electron/chrome-sandbox"

ENGINE_VENV="$PAYLOAD/engine/.venv"
python3 -m venv "$ENGINE_VENV"
"$ENGINE_VENV/bin/pip" install --disable-pip-version-check \
  -r "$ENGINE_ROOT/requirements.txt" \
  -r "$ENGINE_ROOT/requirements-gemini.txt" \
  -r "$ENGINE_ROOT/requirements-bedrock.txt"
find "$ENGINE_VENV" -type d -name __pycache__ -prune -exec rm -rf {} +

install -m 0755 "$ROOT/tools/intentctl.py" "$STAGE/usr/bin/intentctl"
install -m 0755 "$ROOT/packaging/debian/intent-launcher" "$STAGE/usr/bin/intent"
install -m 0755 "$ROOT/tools/intent_workspace_watch.py" "$STAGE/usr/bin/intent-workspace-watch"
install -m 0755 "$ROOT/shell/intent_shell_send.py" "$STAGE/usr/bin/intent-shell-send"
install -m 0644 "$ROOT/packaging/debian/intent.desktop" "$STAGE/usr/share/applications/intent.desktop"
install -m 0644 "$ROOT/packaging/debian/icons/intent.svg" "$STAGE/usr/share/icons/hicolor/scalable/apps/intent.svg"
install -m 0644 "$ROOT/packaging/debian/systemd/intent-server.service" "$STAGE/usr/lib/systemd/user/"
install -m 0644 "$ROOT/packaging/debian/systemd/intent-engine.service" "$STAGE/usr/lib/systemd/user/"
install -m 0644 "$ROOT/packaging/debian/systemd/intent-x11-tracker.service" "$STAGE/usr/lib/systemd/user/"
install -m 0644 "$ROOT/packaging/debian/systemd/intent-workspace-watch.service" "$STAGE/usr/lib/systemd/user/"
install -m 0644 "$ROOT/packaging/debian/systemd/intent-pipeline.service" "$STAGE/usr/lib/systemd/user/"
install -m 0644 "$ROOT/packaging/debian/systemd/intent-pipeline.timer" "$STAGE/usr/lib/systemd/user/"
install -m 0644 "$ROOT/packaging/debian/systemd/intent-backend.target" "$STAGE/usr/lib/systemd/user/"
sed \
  -e "s/@ARCHITECTURE@/$ARCHITECTURE/g" \
  -e "s/@ENGINE_PYTHON_PACKAGE@/$PYTHON_PACKAGE/g" \
  "$ROOT/packaging/debian/control" > "$STAGE/DEBIAN/control"
install -m 0755 "$ROOT/packaging/debian/maintainer-scripts/postinst" "$STAGE/DEBIAN/postinst"
install -m 0755 "$ROOT/packaging/debian/maintainer-scripts/prerm" "$STAGE/DEBIAN/prerm"
install -m 0755 "$ROOT/packaging/debian/maintainer-scripts/postrm" "$STAGE/DEBIAN/postrm"

mkdir -p "$(dirname "$OUTPUT")"
dpkg-deb --build --root-owner-group "$STAGE" "$OUTPUT"
echo "Built $OUTPUT"
