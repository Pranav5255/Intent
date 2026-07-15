#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
MODE=release
OUTPUT="$ROOT/dist/intent-os_0.1.0_all.deb"
FIREFOX_XPI="${INTENT_OS_FIREFOX_XPI:-}"

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

STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
PAYLOAD="$STAGE/opt/intent-os"
mkdir -p "$PAYLOAD" "$STAGE/DEBIAN" "$STAGE/usr/bin" "$STAGE/usr/lib/systemd/user"

cp -a "$ROOT/event_server" "$ROOT/collectors" "$ROOT/shell" "$ROOT/integrations" \
  "$ROOT/fixtures" "$ROOT/scripts" "$ROOT/tools" "$ROOT/packaging" "$PAYLOAD/"
find "$PAYLOAD" -type d \( -name tests -o -name __pycache__ \) -prune -exec rm -rf {} +
rm -f "$PAYLOAD/integrations/firefox-extension/dist/intent-os-firefox.xpi"
install -D -m 0644 "$FIREFOX_XPI" "$PAYLOAD/integrations/firefox-extension/dist/intent-os-firefox.xpi"
printf '%s\n' "$MODE" > "$PAYLOAD/BUILD_MODE"

install -m 0755 "$ROOT/tools/intent_osctl.py" "$STAGE/usr/bin/intent-osctl"
install -m 0755 "$ROOT/tools/intent_os_workspace_watch.py" "$STAGE/usr/bin/intent-os-workspace-watch"
install -m 0755 "$ROOT/shell/intent_os_shell_send.py" "$STAGE/usr/bin/intent-os-shell-send"
install -m 0644 "$ROOT/packaging/debian/systemd/intent-os-server.service" "$STAGE/usr/lib/systemd/user/"
install -m 0644 "$ROOT/packaging/debian/systemd/intent-os-x11-tracker.service" "$STAGE/usr/lib/systemd/user/"
install -m 0644 "$ROOT/packaging/debian/systemd/intent-os-workspace-watch.service" "$STAGE/usr/lib/systemd/user/"
cp "$ROOT/packaging/debian/control" "$STAGE/DEBIAN/control"
install -m 0755 "$ROOT/packaging/debian/maintainer-scripts/postinst" "$STAGE/DEBIAN/postinst"
install -m 0755 "$ROOT/packaging/debian/maintainer-scripts/prerm" "$STAGE/DEBIAN/prerm"
install -m 0755 "$ROOT/packaging/debian/maintainer-scripts/postrm" "$STAGE/DEBIAN/postrm"

mkdir -p "$(dirname "$OUTPUT")"
dpkg-deb --build --root-owner-group "$STAGE" "$OUTPUT"
echo "Built $OUTPUT ($MODE mode)"
