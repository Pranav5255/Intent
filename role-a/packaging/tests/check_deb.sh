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
test -f "$WORK/usr/lib/systemd/user/intent-os-role-b.service"
test -f "$WORK/usr/lib/systemd/user/intent-os-x11-tracker.service"
test -f "$WORK/usr/lib/systemd/user/intent-os-workspace-watch.service"
test -f "$WORK/opt/intent-os/integrations/firefox-extension/dist/intent-os-firefox.xpi"
grep -Fq 'file:///etc/firefox/policies/intent-os-firefox.xpi' "$WORK/opt/intent-os/packaging/debian/firefox-policies.json"
test -f "$WORK/opt/intent-os/integrations/vscode-extension/dist/intent-os-vscode.vsix"
test -f "$WORK/opt/intent-os/role-b/intent_engine/api.py"
test -x "$WORK/opt/intent-os/role-b/.venv/bin/python"
grep -Fq 'intent_engine.api:app --host 127.0.0.1 --port 9478' "$WORK/usr/lib/systemd/user/intent-os-role-b.service"
PYTHONPATH="$WORK/opt/intent-os/role-b" "$WORK/opt/intent-os/role-b/.venv/bin/python" -c 'from intent_engine.api import app; assert app.title == "Intent OS - Role B"'
echo "Package structure OK: $PACKAGE"
