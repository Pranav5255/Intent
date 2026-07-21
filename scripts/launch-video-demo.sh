#!/usr/bin/env bash
set -euo pipefail

# Launch an isolated, recording-ready Intent demo.  It never reads or
# writes the normal Intent databases or uses the installed user services.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROLE_A="$ROOT/role-a"
ROLE_B="$ROOT/role-b"
ROLE_C="$ROOT/role-c/app"

SCENARIO="${1:-$HOME/demo-vid-scenarios.json}"
RUNTIME="${INTENT_OS_DEMO_RUNTIME:-$(mktemp -d /tmp/intent-os-video-demo-XXXXXX)}"
ROLE_A_PORT="${INTENT_OS_DEMO_ROLE_A_PORT:-9587}"
ROLE_B_PORT="${INTENT_OS_DEMO_ROLE_B_PORT:-9588}"
RENDERER_PORT="${INTENT_OS_DEMO_RENDERER_PORT:-9589}"
SHORTCUT="${INTENT_OS_DEMO_SHORTCUT:-Control+Space}"

require_file() {
  local path="$1"
  local description="$2"
  if [[ ! -f "$path" ]]; then
    printf 'Missing %s: %s\n' "$description" "$path" >&2
    exit 1
  fi
}

wait_for_url() {
  local url="$1"
  for _ in $(seq 1 40); do
    if curl -fsS "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  printf 'Timed out waiting for %s\n' "$url" >&2
  return 1
}

launch_role_a() {
  (
    cd "$ROLE_A"
    exec setsid env INTENT_OS_DATABASE="$RUNTIME/events.db" \
      "$ROLE_A/.venv/bin/python" -m uvicorn event_server.main:app \
      --host 127.0.0.1 --port "$ROLE_A_PORT"
  ) >"$RUNTIME/logs/role-a.log" 2>&1 &
  echo "$!" >"$RUNTIME/role-a.pid"
}

launch_role_b() {
  local llm_enabled="$1"
  (
    cd "$ROLE_B"
    exec setsid env \
      ROLE_B_DB_PATH="$RUNTIME/intents.db" \
      INTENT_OS_ROLE_A_URL="http://127.0.0.1:$ROLE_A_PORT" \
      LLM_PROVIDER=gemini \
      GEMINI_CREDENTIALS_PATH="$GEMINI_CREDENTIALS_PATH" \
      GOOGLE_APPLICATION_CREDENTIALS="$GEMINI_CREDENTIALS_PATH" \
      ROLE_B_LLM_ENABLED="$llm_enabled" \
      ENABLE_COPILOT="$llm_enabled" \
      ROLE_B_SEMANTIC_CLUSTER=false \
      "$ROLE_B/.venv/bin/python" -m uvicorn intent_engine.api:app \
      --host 127.0.0.1 --port "$ROLE_B_PORT"
  ) >"$RUNTIME/logs/role-b.log" 2>&1 &
  echo "$!" >"$RUNTIME/role-b.pid"
}

launch_renderer() {
  (
    cd "$ROLE_C"
    exec setsid env \
      VITE_ROLE_A_URL="http://127.0.0.1:$ROLE_A_PORT" \
      VITE_ROLE_B_URL="http://127.0.0.1:$ROLE_B_PORT" \
      VITE_INTENT_DATE="$FIXTURE_DATE" \
      "$ROLE_C/node_modules/.bin/vite" --host 127.0.0.1 --port "$RENDERER_PORT" --strictPort
  ) >"$RUNTIME/logs/renderer.log" 2>&1 &
  echo "$!" >"$RUNTIME/renderer.pid"
}

launch_electron() {
  (
    cd "$ROLE_C"
    exec setsid env \
      VITE_DEV_SERVER_URL="http://127.0.0.1:$RENDERER_PORT?demo=1" \
      INTENT_OS_ROLE_A_URL="http://127.0.0.1:$ROLE_A_PORT" \
      INTENT_OS_ROLE_B_URL="http://127.0.0.1:$ROLE_B_PORT" \
      INTENT_OS_OVERLAY_SHORTCUT="$SHORTCUT" \
      "$ROLE_C/node_modules/.bin/electron" --user-data-dir="$RUNTIME/electron-profile" .
  ) >"$RUNTIME/logs/electron.log" 2>&1 &
  echo "$!" >"$RUNTIME/electron.pid"
}

require_file "$SCENARIO" "demo scenario"
require_file "${GEMINI_CREDENTIALS_PATH:-}" "GEMINI_CREDENTIALS_PATH"
require_file "$ROLE_A/.venv/bin/python" "Role A Python environment"
require_file "$ROLE_B/.venv/bin/python" "Role B Python environment"
require_file "$ROLE_C/node_modules/.bin/electron" "Electron runtime"
require_file "$ROLE_C/node_modules/.bin/vite" "Vite runtime"

FIXTURE_DATE="$(jq -er '.date | strings' "$SCENARIO")"
mkdir -p "$RUNTIME/logs"

launch_role_a
wait_for_url "http://127.0.0.1:$ROLE_A_PORT/healthz"

# Replay the supplied scenario before applying the isolated recording focus.
launch_role_b false
wait_for_url "http://127.0.0.1:$ROLE_B_PORT/healthz"
curl --fail-with-body -sS -X POST "http://127.0.0.1:$ROLE_B_PORT/pipeline/run-replay" \
  -H 'content-type: application/json' \
  --data-binary "@$SCENARIO" >"$RUNTIME/pipeline-result.json"

# Restart only the isolated Role B process with Gemini ready for ? questions.
kill "$(cat "$RUNTIME/role-b.pid")"
wait "$(cat "$RUNTIME/role-b.pid")" 2>/dev/null || true
PYTHONPATH="$ROLE_B" "$ROLE_B/.venv/bin/python" "$ROOT/scripts/focus-video-demo.py" \
  --database "$RUNTIME/intents.db" --date "$FIXTURE_DATE" >"$RUNTIME/focus-result.txt"
launch_role_b true
wait_for_url "http://127.0.0.1:$ROLE_B_PORT/healthz"
curl --fail-with-body -sS "http://127.0.0.1:$ROLE_B_PORT/settings/llm" >"$RUNTIME/llm-status.json"

demo_headline="$(curl -fsS "http://127.0.0.1:$ROLE_B_PORT/intents/digest?date=$FIXTURE_DATE" | jq -r '.headline')"
if [[ "$demo_headline" != "Building Login Feature" ]]; then
  echo 'Demo focus did not produce the expected login headline.' >&2
  exit 1
fi
demo_search_count="$(curl -fsS "http://127.0.0.1:$ROLE_B_PORT/intents/search?q=jwt&limit=1" | jq 'length')"
if [[ "$demo_search_count" -lt 1 ]]; then
  echo 'Demo focus did not make the JWT session searchable.' >&2
  exit 1
fi

launch_renderer
wait_for_url "http://127.0.0.1:$RENDERER_PORT"
launch_electron

printf '\nIntent video demo is ready.\n'
printf 'Runtime: %s\n' "$RUNTIME"
printf 'Fixture date: %s\n' "$FIXTURE_DATE"
printf 'Hotkey: %s\n' "$SHORTCUT"
printf 'Search: jwt\n'
printf 'Copilot: ? What was I working on yesterday?\n'
printf 'Logs: %s/logs\n' "$RUNTIME"
