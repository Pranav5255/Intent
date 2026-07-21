#!/usr/bin/env bash
set -euo pipefail

PACKAGE=${1:?Usage: check_deb.sh path/to/intent-os.deb}
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

dpkg-deb --info "$PACKAGE" >/dev/null
dpkg-deb --extract "$PACKAGE" "$WORK"
test -x "$WORK/usr/bin/intent-osctl"
test -x "$WORK/usr/bin/intent-os"
test -x "$WORK/usr/bin/intent-os-shell-send"
test -f "$WORK/usr/share/applications/intent-os.desktop"
test -f "$WORK/usr/share/icons/hicolor/scalable/apps/intent-os.svg"
test -f "$WORK/usr/lib/systemd/user/intent-os-server.service"
test -f "$WORK/usr/lib/systemd/user/intent-os-role-b.service"
test -f "$WORK/usr/lib/systemd/user/intent-os-x11-tracker.service"
test -f "$WORK/usr/lib/systemd/user/intent-os-workspace-watch.service"
test -f "$WORK/usr/lib/systemd/user/intent-os-backend.target"
test -f "$WORK/usr/lib/systemd/user/intent-os-pipeline.service"
test -f "$WORK/usr/lib/systemd/user/intent-os-pipeline.timer"
test -f "$WORK/opt/intent-os/integrations/firefox-extension/dist/intent-os-firefox.xpi"
grep -Fq 'file:///etc/firefox/policies/intent-os-firefox.xpi' "$WORK/opt/intent-os/packaging/debian/firefox-policies.json"
test -f "$WORK/opt/intent-os/integrations/vscode-extension/dist/intent-os-vscode.vsix"
test -f "$WORK/opt/intent-os/role-b/intent_engine/api.py"
test -x "$WORK/opt/intent-os/role-b/.venv/bin/python"
test -f "$WORK/opt/intent-os/role-c/app/package.json"
test -f "$WORK/opt/intent-os/role-c/app/electron/main.cjs"
test -f "$WORK/opt/intent-os/role-c/app/dist/index.html"
test -x "$WORK/opt/intent-os/role-c/electron/electron"
test "$(stat -c %a "$WORK/opt/intent-os/role-c/electron/chrome-sandbox")" = 4755
grep -Fq 'intent-osctl enable' "$WORK/usr/bin/intent-os"
grep -Fq 'intent-osctl wait-ready' "$WORK/usr/bin/intent-os"
grep -Fq 'unset ELECTRON_RUN_AS_NODE' "$WORK/usr/bin/intent-os"
grep -Fq 'unset VITE_DEV_SERVER_URL' "$WORK/usr/bin/intent-os"
grep -Fq '/opt/intent-os/role-c/electron/electron /opt/intent-os/role-c/app' "$WORK/usr/bin/intent-os"
grep -Fq 'Exec=intent-os' "$WORK/usr/share/applications/intent-os.desktop"
grep -Fq 'Icon=intent-os' "$WORK/usr/share/applications/intent-os.desktop"
grep -Fq 'StartupWMClass=intent-os-role-c' "$WORK/usr/share/applications/intent-os.desktop"
test ! -d "$WORK/opt/intent-os/role-c/app/node_modules"
grep -Fq 'intent_engine.api:app --host 127.0.0.1 --port 9478' "$WORK/usr/lib/systemd/user/intent-os-role-b.service"
grep -Fq 'INTENT_OS_DATABASE=%h/.local/share/intent-os/events.db' "$WORK/usr/lib/systemd/user/intent-os-server.service"
grep -Fq 'intent_engine.scheduled_ingest' "$WORK/usr/lib/systemd/user/intent-os-pipeline.service"
grep -Fq 'Environment=ENABLE_PIPELINE_TRIGGER=true' "$WORK/usr/lib/systemd/user/intent-os-pipeline.service"
grep -Fq 'ROLE_B_DB_PATH=%h/.local/share/intent-os/intents.db' "$WORK/usr/lib/systemd/user/intent-os-pipeline.service"
grep -Fq 'OnCalendar=*-*-* 00/3:00:00' "$WORK/usr/lib/systemd/user/intent-os-pipeline.timer"
grep -Fq 'Persistent=true' "$WORK/usr/lib/systemd/user/intent-os-pipeline.timer"
grep -Fq 'intent-os-pipeline.timer' "$WORK/usr/lib/systemd/user/intent-os-backend.target"
ROLE_B_DB_PATH="$WORK/role-b-check.db" PYTHONPATH="$WORK/opt/intent-os/role-b" "$WORK/opt/intent-os/role-b/.venv/bin/python" -c 'from intent_engine.api import app; assert app.title == "Intent OS - Role B"'
echo "Package structure OK: $PACKAGE"
