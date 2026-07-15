#!/usr/bin/env bash
set -euo pipefail

PACKAGE=${1:?Usage: check_deb.sh path/to/intent-os.deb}
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

dpkg-deb --info "$PACKAGE" >/dev/null
dpkg-deb --extract "$PACKAGE" "$WORK"
test -x "$WORK/usr/bin/intent-osctl"
test -x "$WORK/usr/bin/intent-os-shell-send"
test -f "$WORK/usr/lib/systemd/user/intent-os-server.service"
test -f "$WORK/usr/lib/systemd/user/intent-os-x11-tracker.service"
test -f "$WORK/usr/lib/systemd/user/intent-os-workspace-watch.service"
test -f "$WORK/opt/intent-os/integrations/firefox-extension/dist/intent-os-firefox.xpi"
grep -Fq 'file:///etc/firefox/policies/intent-os-firefox.xpi' "$WORK/opt/intent-os/packaging/debian/firefox-policies.json"
test -f "$WORK/opt/intent-os/integrations/vscode-extension/dist/intent-os-vscode.vsix"
echo "Package structure OK: $PACKAGE"
