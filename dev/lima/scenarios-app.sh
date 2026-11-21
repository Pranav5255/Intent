#!/usr/bin/env bash
# Run the desktop app from source in the VM's Xvfb session with the Reticle SDK, so a
# coding agent can drive the real app against the installed daemon.
#   On the Mac:  npx @reticlehq/server serve        (once; listens on 127.0.0.1:4400)
#                ssh -F ~/.lima/intent/ssh.config -N -R 127.0.0.1:4400:127.0.0.1:4400 lima-intent
#   In the VM:   dev/lima/scenarios-app.sh [repo-copy]   (default ~/work/Intent)
#
# The app runs from a copy on the VM's own disk, so Linux node_modules stay off the Mac.
# The Reticle packages are installed with --no-save: they are never in package.json.
# The UI scenarios themselves are listed in docs/TESTING.md.
set -euo pipefail

SRC=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
WORK=${1:-$HOME/work/Intent}
RETICLE_VERSION=${RETICLE_VERSION:-3.10.10}
LOG=${LOG:-$HOME/work/tauri-dev.log}
export DISPLAY=${DISPLAY:-:99} XDG_SESSION_TYPE=x11

pgrep -x Xvfb >/dev/null || { Xvfb "$DISPLAY" -screen 0 1280x800x24 -nolisten tcp >/dev/null 2>&1 & sleep 1; }
pgrep -x openbox >/dev/null || { openbox >/dev/null 2>&1 & sleep 1; }

mkdir -p "$WORK"
rsync -a --delete --exclude target --exclude node_modules --exclude .git "$SRC/" "$WORK/"
cd "$WORK/app"
[[ -d node_modules ]] || npm ci --no-audit --no-fund
npm i --no-save --no-audit --no-fund "@reticlehq/vite-plugin@$RETICLE_VERSION" "@reticlehq/react@$RETICLE_VERSION" >/dev/null
rm -rf node_modules/.vite   # a cached entry module skips the SDK injection

pkill -x intent-app || true
pkill -f "[t]auri dev" || true
fuser -k 1420/tcp 2>/dev/null || true
sleep 1

# The pairing token lives in the Mac home, which Lima mounts read-only.
RETICLE=1 RETICLE_PORT=4400 RETICLE_PAIRING_TOKEN_DIR=${RETICLE_PAIRING_TOKEN_DIR:-$(dirname "$(ls /Users/*/.reticle/pairing-token | head -1)")} \
  setsid nohup npx tauri dev --config src-tauri/tauri.reticle.conf.json >"$LOG" 2>&1 </dev/null &
for _ in $(seq 120); do pgrep -f target/debug/intent-app >/dev/null && break; sleep 2; done
pgrep -f target/debug/intent-app >/dev/null || { tail -20 "$LOG"; exit 1; }
echo "app running; log: $LOG"
