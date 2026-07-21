#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
ROLE_B_ROOT=$(cd "$ROOT/../role-b" && pwd)
ROLE_C_ROOT=$(cd "$ROOT/../role-c/app" && pwd)
MODE=release
VERSION=0.1.8
ARCHITECTURE=$(dpkg --print-architecture)
PYTHON_VERSION=$(python3 -c 'import sys; print(f"{sys.version_info.major}.{sys.version_info.minor}")')
PYTHON_PACKAGE="python${PYTHON_VERSION}"
OUTPUT="$ROOT/dist/intent-os_${VERSION}_${ARCHITECTURE}.deb"
FIREFOX_XPI="${INTENT_OS_FIREFOX_XPI:-}"
ROLE_C_ELECTRON="$ROLE_C_ROOT/node_modules/electron/dist"

usage() {
  cat <<'EOF'
Usage: packaging/build-deb.sh [--dev] [--output path]

Release builds require INTENT_OS_FIREFOX_XPI to point to a Mozilla-signed XPI.
--dev uses the local unsigned XPI and deliberately does not install a Firefox policy.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dev) MODE=dev ;;
    --output) OUTPUT=$2; shift ;;
    --help) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

if [[ "$MODE" = dev ]]; then
  FIREFOX_XPI="$ROOT/integrations/firefox-extension/dist/intent-os-firefox.xpi"
elif [[ -z "$FIREFOX_XPI" ]]; then
  echo "Release build requires INTENT_OS_FIREFOX_XPI=/path/to/signed.xpi" >&2
  exit 2
fi
[[ -f "$FIREFOX_XPI" ]] || { echo "Firefox XPI not found: $FIREFOX_XPI" >&2; exit 2; }
[[ -f "$ROOT/integrations/vscode-extension/dist/intent-os-vscode.vsix" ]] || {
  echo "Build the bundled VSIX before packaging." >&2
  exit 2
}
[[ -d "$ROLE_B_ROOT/intent_engine" ]] || { echo "Role B source is unavailable: $ROLE_B_ROOT" >&2; exit 2; }
[[ -x "$ROLE_C_ELECTRON/electron" ]] || {
  echo "Role C Electron runtime is unavailable; run npm ci in role-c/app first." >&2
  exit 2
}
[[ -x "$ROLE_C_ROOT/node_modules/.bin/vite" ]] || {
  echo "Role C build tooling is unavailable; run npm ci in role-c/app first." >&2
  exit 2
}
python3 -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 11) else 1)' || {
  echo "Role B packaging requires Python 3.11 or newer." >&2
  exit 2
}

# The renderer is compiled as part of the unified artifact. The packaged
# Electron app contains only its production files and runtime -- no npm
# development tree is installed on the user's machine.
npm --prefix "$ROLE_C_ROOT" run build

STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
PAYLOAD="$STAGE/opt/intent-os"
mkdir -p "$PAYLOAD" "$STAGE/DEBIAN" "$STAGE/usr/bin" "$STAGE/usr/lib/systemd/user" \
  "$STAGE/usr/share/applications" "$STAGE/usr/share/icons/hicolor/scalable/apps"

cp -a "$ROOT/event_server" "$ROOT/collectors" "$ROOT/shell" "$ROOT/integrations" \
  "$ROOT/fixtures" "$ROOT/scripts" "$ROOT/tools" "$ROOT/packaging" "$PAYLOAD/"
mkdir -p "$PAYLOAD/role-b"
cp -a "$ROLE_B_ROOT/intent_engine" "$ROLE_B_ROOT/docs" "$PAYLOAD/role-b/"
install -m 0755 "$ROLE_B_ROOT/mcp_server.py" "$PAYLOAD/role-b/mcp_server.py"
install -m 0644 "$ROLE_B_ROOT/.env.example" "$ROLE_B_ROOT/README.md" \
  "$ROLE_B_ROOT/requirements.txt" "$ROLE_B_ROOT/requirements-openai.txt" \
  "$ROLE_B_ROOT/requirements-gemini.txt" "$ROLE_B_ROOT/requirements-bedrock.txt" "$PAYLOAD/role-b/"
find "$PAYLOAD" -type d \( -name tests -o -name __pycache__ \) -prune -exec rm -rf {} +
rm -f "$PAYLOAD/integrations/firefox-extension/dist/intent-os-firefox.xpi"
install -D -m 0644 "$FIREFOX_XPI" "$PAYLOAD/integrations/firefox-extension/dist/intent-os-firefox.xpi"
printf '%s\n' "$MODE" > "$PAYLOAD/BUILD_MODE"

mkdir -p "$PAYLOAD/role-c/app" "$PAYLOAD/role-c/electron"
cp -a "$ROLE_C_ROOT/dist" "$ROLE_C_ROOT/electron" "$ROLE_C_ROOT/package.json" "$PAYLOAD/role-c/app/"
cp -a "$ROLE_C_ELECTRON/." "$PAYLOAD/role-c/electron/"
# Electron uses the setuid helper when user namespaces are unavailable. The
# package builder assigns root ownership with --root-owner-group below.
chmod 4755 "$PAYLOAD/role-c/electron/chrome-sandbox"

ROLE_B_VENV="$PAYLOAD/role-b/.venv"
python3 -m venv "$ROLE_B_VENV"
"$ROLE_B_VENV/bin/pip" install --disable-pip-version-check \
  -r "$ROLE_B_ROOT/requirements.txt" \
  -r "$ROLE_B_ROOT/requirements-gemini.txt" \
  -r "$ROLE_B_ROOT/requirements-bedrock.txt"
find "$ROLE_B_VENV" -type d -name __pycache__ -prune -exec rm -rf {} +

install -m 0755 "$ROOT/tools/intent_osctl.py" "$STAGE/usr/bin/intent-osctl"
install -m 0755 "$ROOT/packaging/debian/intent-os-launcher" "$STAGE/usr/bin/intent-os"
install -m 0755 "$ROOT/tools/intent_os_workspace_watch.py" "$STAGE/usr/bin/intent-os-workspace-watch"
install -m 0755 "$ROOT/shell/intent_os_shell_send.py" "$STAGE/usr/bin/intent-os-shell-send"
install -m 0644 "$ROOT/packaging/debian/intent-os.desktop" "$STAGE/usr/share/applications/intent-os.desktop"
install -m 0644 "$ROOT/packaging/debian/icons/intent-os.svg" "$STAGE/usr/share/icons/hicolor/scalable/apps/intent-os.svg"
install -m 0644 "$ROOT/packaging/debian/systemd/intent-os-server.service" "$STAGE/usr/lib/systemd/user/"
install -m 0644 "$ROOT/packaging/debian/systemd/intent-os-role-b.service" "$STAGE/usr/lib/systemd/user/"
install -m 0644 "$ROOT/packaging/debian/systemd/intent-os-x11-tracker.service" "$STAGE/usr/lib/systemd/user/"
install -m 0644 "$ROOT/packaging/debian/systemd/intent-os-workspace-watch.service" "$STAGE/usr/lib/systemd/user/"
install -m 0644 "$ROOT/packaging/debian/systemd/intent-os-pipeline.service" "$STAGE/usr/lib/systemd/user/"
install -m 0644 "$ROOT/packaging/debian/systemd/intent-os-pipeline.timer" "$STAGE/usr/lib/systemd/user/"
install -m 0644 "$ROOT/packaging/debian/systemd/intent-os-backend.target" "$STAGE/usr/lib/systemd/user/"
sed \
  -e "s/@ARCHITECTURE@/$ARCHITECTURE/g" \
  -e "s/@ROLE_B_PYTHON_PACKAGE@/$PYTHON_PACKAGE/g" \
  "$ROOT/packaging/debian/control" > "$STAGE/DEBIAN/control"
install -m 0755 "$ROOT/packaging/debian/maintainer-scripts/postinst" "$STAGE/DEBIAN/postinst"
install -m 0755 "$ROOT/packaging/debian/maintainer-scripts/prerm" "$STAGE/DEBIAN/prerm"
install -m 0755 "$ROOT/packaging/debian/maintainer-scripts/postrm" "$STAGE/DEBIAN/postrm"

mkdir -p "$(dirname "$OUTPUT")"
dpkg-deb --build --root-owner-group "$STAGE" "$OUTPUT"
echo "Built $OUTPUT ($MODE mode)"
