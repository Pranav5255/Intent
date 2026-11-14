#!/usr/bin/env bash
# End-to-end check of the Intent 2.0 .deb inside the Lima dev VM:
#   limactl shell intent -- bash -lc '<repo>/dev/lima/e2e.sh [path/to/intent.deb]'
#
# Installs the package, starts a headless X session (Xvfb + openbox), enables the
# user service, generates real focus / shell / file activity plus companion
# events, runs the pipeline and checks the API. Exits non-zero on any failure.
set -euo pipefail

DEB=${1:-$HOME/dist/intent_2.0.0_$(dpkg --print-architecture).deb}
API=http://127.0.0.1:9477
export DISPLAY=:99 XDG_SESSION_TYPE=x11
WS=$HOME/e2e/intent-demo
TODAY=$(date +%F)

step() { printf '\n== %s\n' "$*"; }
fail() { echo "E2E FAILED: $*" >&2; exit 1; }
uuid() { cat /proc/sys/kernel/random/uuid; }

step "install $DEB"
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --reinstall "$DEB" >/dev/null
dpkg -s intent | grep -E '^(Version|Depends|Installed-Size)'
command -v xterm >/dev/null || sudo DEBIAN_FRONTEND=noninteractive apt-get install -y xterm >/dev/null

step "headless X session"
pgrep -x Xvfb >/dev/null || { Xvfb :99 -screen 0 1280x800x24 -nolisten tcp >/dev/null 2>&1 & sleep 1; }
pgrep -x openbox >/dev/null || { openbox >/dev/null 2>&1 & sleep 1; }
xdpyinfo -display :99 >/dev/null || fail "X server not up"

step "intentctl enable"
intentctl enable
for _ in $(seq 50); do curl -fs "$API/healthz" >/dev/null && break; sleep 0.2; done
curl -fs "$API/healthz" | jq -c .
systemctl --user is-active intentd.service
TOKEN=$(cat "$XDG_RUNTIME_DIR/intent/token")
AUTH=(-H "Authorization: Bearer $TOKEN")
api() { curl -fsS "${AUTH[@]}" "$@"; }
[[ $(curl -s -o /dev/null -w '%{http_code}' "$API/v1/status") == 401 ]] || fail "status must need the token"

step "approve a workspace (applied live)"
rm -rf "$WS" && mkdir -p "$WS/src"
intentctl workspace add "$WS"
for _ in $(seq 30); do
  api "$API/v1/status" | jq -e '.collectors[] | select(.name=="filesystem") | .detail.watched_dirs > 0' >/dev/null && break
  sleep 0.5
done
sleep 2

step "focus switches between windows"
setsid xterm -title e2e-editor -e sleep 900 </dev/null >/dev/null 2>&1 &
setsid xterm -title e2e-docs -e sleep 900 </dev/null >/dev/null 2>&1 &
sleep 2
for _ in 1 2 3 4 5; do
  xdotool search --name e2e-editor windowactivate --sync; sleep 1.5
  xdotool search --name e2e-docs windowactivate --sync; sleep 1.5
done

step "interactive bash with the hook"
bash -i >/dev/null 2>&1 <<EOF
cd "$WS"
git init -q .
ls -la
cargo --version
false
echo done
exit
EOF
sleep 3

step "file changes in the approved workspace"
for i in 1 2 3 4; do
  printf 'pub fn f%s() {}\n' "$i" >> "$WS/src/lib.rs"
  printf '# notes %s\n' "$i" > "$WS/NOTES$i.md"
  sleep 1
done
sleep 14   # debounce (2 s) + per-path coalescing (10 s)

step "companion events (Firefox + VS Code payloads)"
now=$(date +%s)
post() { curl -s -o /dev/null -w '%{http_code} ' -H 'content-type: application/json' -d "$1" "$API/v1/event"; }
post "{\"id\":\"$(uuid)\",\"ts\":$now,\"source\":\"firefox\",\"type\":\"tab_change\",\"payload\":{\"url\":\"https://docs.rs/axum/latest/axum/\",\"title\":\"axum - Rust\",\"tab_id\":3,\"window_id\":1}}"
post "{\"id\":\"$(uuid)\",\"ts\":$now,\"source\":\"firefox\",\"type\":\"tab_change\",\"payload\":{\"url\":\"https://doc.rust-lang.org/book/\",\"title\":\"The Rust Book\",\"tab_id\":4,\"window_id\":1}}"
post "{\"id\":\"$(uuid)\",\"schema_version\":1,\"ts\":$now,\"source\":\"vscode\",\"type\":\"workspace_open\",\"payload\":{\"folder\":\"$WS\"}}"
post "{\"id\":\"$(uuid)\",\"schema_version\":1,\"ts\":$now,\"source\":\"vscode\",\"type\":\"file_open\",\"payload\":{\"path\":\"$WS/src/lib.rs\"}}"
post "{\"id\":\"$(uuid)\",\"schema_version\":1,\"ts\":$((now+20)),\"source\":\"vscode\",\"type\":\"file_save\",\"payload\":{\"path\":\"$WS/src/lib.rs\"}}"
echo

step "pipeline"
intentctl run

step "verify"
status=$(api "$API/v1/status")
echo "$status" | jq -c '{paused: .capture_paused, sources: (.sources | map_values(.event_count)), collectors: [.collectors[] | {name, state, backend, events_emitted}], ingestion: (.ingestion | {inserted, duplicates, dropped_by_reason, batches, average_write_ms}), llm: (.llm | {provider, reachable}), db_bytes: .database.size_bytes, last_run: .pipeline.last_run}'
for src in linux shell filesystem firefox vscode; do
  n=$(echo "$status" | jq ".sources.$src.event_count")
  [[ $n -gt 0 ]] || fail "no $src events captured"
done
echo "$status" | jq -e '.collectors[] | select(.name=="focus") | .state == "running"' >/dev/null || fail "focus collector not running"
api "$API/v1/events?date=$TODAY" | jq -r '.[] | select(.source=="shell") | .payload.cmd' | sort -u | sed 's/^/  shell: /'
api "$API/v1/events?date=$TODAY" | jq -r '.[] | select(.source=="linux" and .type=="app_focus") | .payload.title' | sort | uniq -c | sed 's/^/  focus:/'
intents=$(api "$API/intents?date=$TODAY")
echo "$intents" | jq -r '.[] | "  intent: \(.label) — \(.summary) [\(.stats.event_count) events, tags \(.tags | join(","))]"'
[[ $(echo "$intents" | jq length) -gt 0 ]] || fail "no intents"
word=$(echo "$intents" | jq -r '.[0].label' | awk '{print $1}')
api "$API/intents/search?q=$word" | jq -c '[.[] | {label, score}]'
api "$API/intents/digest?date=$TODAY" | jq -c '{headline, intent_count, total_duration_seconds}'
api -X POST "$API/resume/select" -H 'content-type: application/json' -d "{\"query\":\"$word\"}" | jq -c '{needs_picker, n: (.candidates|length), selected: .selected.label}'
pkill -f "xterm -title e2e-" || true
echo; echo "E2E OK"
