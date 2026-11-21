#!/usr/bin/env bash
# User-behaviour scenarios against a throwaway intentd. Each scenario acts like a
# user (or a hostile page, or a broken environment) and asserts what Intent must do.
#   dev/lima/scenarios.sh                 # every scenario
#   dev/lima/scenarios.sh forget pause    # only scenarios whose name contains a word
#
# The daemon under test runs on 127.0.0.1:9478 with its own HOME and XDG dirs, so the
# installed service and its data are not touched. Set INTENTD / INTENTCTL to test a
# build other than the installed one. Exits non-zero when any scenario breaks.
set -uo pipefail

INTENTD=${INTENTD:-$(command -v intentd)}
INTENTCTL=${INTENTCTL:-$(command -v intentctl)}
PORT=${SCENARIO_PORT:-9478}
API=http://127.0.0.1:$PORT
RUN=$(mktemp -d "${TMPDIR:-/tmp}/intent-scenarios.XXXXXX")
FILTER=("$@")

pass=0
broken=()
DAEMON_PID=

cleanup() {
  stop_daemon
  rm -rf "$RUN"
}
trap cleanup EXIT

# GNU date and /proc on Linux; BSD date and uuidgen on macOS.
if [[ -r /proc/sys/kernel/random/uuid ]]; then
  uuid() { cat /proc/sys/kernel/random/uuid; }
else
  uuid() { uuidgen | tr '[:upper:]' '[:lower:]'; }
fi
now() { date +%s; }

ok() { printf '  ok     %s\n' "$1"; pass=$((pass + 1)); }
bad() { printf '  BROKEN %s\n         %s\n' "$1" "$2"; broken+=("$CURRENT: $1"); }
# check <description> <detail-on-failure> <command...>
check() {
  local what=$1 detail=$2
  shift 2
  if "$@"; then ok "$what"; else bad "$what" "$detail"; fi
}

# ---- daemon lifecycle ----------------------------------------------------------

# start_daemon [VAR=value ...]: a fresh home unless KEEP_HOME=1.
start_daemon() {
  stop_daemon
  if [[ ${KEEP_HOME:-0} != 1 ]]; then
    rm -rf "${RUN:?}/home"
    mkdir -p "$RUN/home/.config" "$RUN/home/.local/share" "$RUN/home/run"
    chmod 700 "$RUN/home/run"
  fi
  HOME_DIR=$RUN/home
  env -i PATH="$PATH" HOME="$HOME_DIR" XDG_CONFIG_HOME="$HOME_DIR/.config" \
    XDG_DATA_HOME="$HOME_DIR/.local/share" XDG_RUNTIME_DIR="$HOME_DIR/run" \
    INTENT_BIND=127.0.0.1:$PORT INTENT_DISABLE_COLLECTORS=1 INTENT_NOTIFICATIONS=0 \
    INTENT_LOG=${INTENT_LOG:-warn} "$@" "$INTENTD" >>"$RUN/daemon.log" 2>&1 &
  DAEMON_PID=$!
  for _ in $(seq 100); do
    curl -fs "$API/healthz" >/dev/null 2>&1 && break
    sleep 0.1
  done
  TOKEN=$(cat "$HOME_DIR/run/intent/token" 2>/dev/null)
  # Template labels: deterministic and fast. SCENARIO_LLM=1 keeps the configured model.
  [[ ${SCENARIO_LLM:-0} == 1 ]] || json_put /settings/llm '{"provider":"none"}'
}

stop_daemon() {
  if [[ -n $DAEMON_PID ]]; then
    kill "$DAEMON_PID" 2>/dev/null
    wait "$DAEMON_PID" 2>/dev/null
    DAEMON_PID=
  fi
}

ctl() {
  env HOME="$HOME_DIR" XDG_RUNTIME_DIR="$HOME_DIR/run" XDG_DATA_HOME="$HOME_DIR/.local/share" \
    INTENT_URL="$API" "$INTENTCTL" "$@"
}

# ---- HTTP helpers ----------------------------------------------------------------

api() { curl -sS -H "Authorization: Bearer $TOKEN" "$@"; }
code() { curl -s -o /dev/null -w '%{http_code}' "$@"; }
json_put() { api -X PUT -H 'content-type: application/json' -d "$2" "$API$1" >/dev/null; }

# post_event <json> → prints the HTTP status
post_event() { code -H 'content-type: application/json' -d "$1" "$API/v1/event"; }

# ev <ts> <source> <type> <payload-json>
ev() {
  post_event "{\"id\":\"$(uuid)\",\"schema_version\":1,\"ts\":$1,\"source\":\"$2\",\"type\":\"$3\",\"payload\":$4}"
}

tab() { ev "$1" firefox tab_change "{\"url\":\"$2\",\"title\":\"$3\",\"tab_id\":1,\"window_id\":1}" >/dev/null; }

# A short workday on <date-ts-base> around two projects.
workday() {
  local t=$1 ws=$HOME_DIR/src
  for i in $(seq 0 9); do
    tab $((t + i * 30)) "https://docs.rs/axum/latest/axum/$i" "axum routing docs $i"
    ev $((t + i * 30 + 10)) vscode file_save "{\"path\":\"$ws/api/src/routes.rs\"}" >/dev/null
  done
  for i in $(seq 0 9); do
    tab $((t + 1200 + i * 30)) "https://react.dev/learn/$i" "react hooks guide $i"
    ev $((t + 1200 + i * 30 + 10)) vscode file_save "{\"path\":\"$ws/web/src/App.tsx\"}" >/dev/null
  done
}

events_json() { api "$API/v1/events?date=$1"; }
if date -d @0 +%F >/dev/null 2>&1; then
  day_of() { date -d "@$1" +%F; }
else
  day_of() { date -r "$1" +%F; }
fi
run_day() { api -X POST "$API/pipeline/run?date=$1" >/dev/null; }

approve_workspace() {
  mkdir -p "$HOME_DIR/src"
  json_put /v1/workspaces "{\"workspaces\":[\"$HOME_DIR/src\"]}"
}

enable_detailed() {
  json_put /v1/detailed-capture/config \
    "{\"approved_workspaces\":[\"$HOME_DIR/src\"],\"browser\":{\"enabled\":true,\"context_enabled\":true},\"editor\":{\"enabled\":true},\"filesystem\":{\"enabled\":true}}"
}


scenario() {
  CURRENT=$1
  if ((${#FILTER[@]})); then
    local match=0 word
    for word in "${FILTER[@]}"; do [[ $1 == *"$word"* ]] && match=1; done
    ((match)) || return 1
  fi
  printf '\n== %s: %s\n' "$1" "$2"
  return 0
}

# ==== scenarios ===================================================================

if scenario baseline "a normal workday turns into intents"; then
  start_daemon
  approve_workspace
  t=$(($(now) - 7200))
  workday "$t"
  run_day "$(day_of "$t")"
  n=$(api "$API/intents?date=$(day_of "$t")" | jq length)
  check "the day has intents" "got $n intents" test "${n:-0}" -gt 0
fi

if scenario cross-site "a web page posts a fake event to the daemon"; then
  start_daemon
  # What fetch(…, {mode:"no-cors"}) sends from any web page: text/plain, an http(s) Origin.
  s=$(code -H 'Origin: https://evil.example' -H 'content-type: text/plain;charset=UTF-8' \
    -d "{\"id\":\"$(uuid)\",\"ts\":$(now),\"source\":\"firefox\",\"type\":\"tab_change\",\"payload\":{\"url\":\"https://evil.example/login\",\"title\":\"Bank login\"}}" \
    "$API/v1/event")
  check "a cross-site POST is refused" "status $s (the event was accepted)" test "$s" -ge 400
  s=$(code -H 'Origin: https://evil.example' -H 'content-type: application/json' \
    -d "{\"id\":\"$(uuid)\",\"ts\":$(now),\"source\":\"shell\",\"type\":\"command\",\"payload\":{\"cmd\":\"x\",\"cwd\":\"/\",\"exit_code\":0}}" \
    "$API/v1/event")
  check "a POST with a foreign web Origin is refused" "status $s" test "$s" -ge 400
  s=$(code -H 'content-type: application/json' -d "{\"id\":\"$(uuid)\",\"ts\":$(now),\"source\":\"shell\",\"type\":\"command\",\"payload\":{\"cmd\":\"ls\",\"cwd\":\"/\",\"exit_code\":0}}" "$API/v1/event")
  check "the shell hook (no Origin) still posts" "status $s" test "$s" = 201
fi

if scenario dns-rebinding "a rebound hostname reads the open config routes"; then
  start_daemon
  s=$(code -H 'Host: evil.example:9478' "$API/v1/detailed-capture/config")
  check "a request for a foreign Host is refused" "status $s (approved workspace paths readable)" test "$s" -ge 400
  s=$(code "$API/healthz")
  check "a loopback Host still works" "status $s" test "$s" = 200
fi

if scenario forget-day "the user forgets today, then keeps working"; then
  start_daemon
  approve_workspace
  t=$(($(now) - 3600))
  d=$(day_of "$t")
  tab "$t" "https://secret-bank.example/" "secret bank statement"
  tab $((t + 60)) "https://secret-bank.example/2" "secret bank statement 2"
  run_day "$d"
  api -X DELETE "$API/v1/memory/date/$d" >/dev/null
  n=$(events_json "$d" | jq '[.[] | select(tostring | test("secret-bank"))] | length')
  check "the day's raw events are deleted" "$n raw events still mention secret-bank" test "${n:-1}" = 0
  tab $((t + 120)) "https://example.org/" "unrelated page"
  run_day "$d"
  n=$(api "$API/intents?date=$d" | jq '[.[] | select(tostring | test("secret"))] | length')
  check "forgotten activity does not come back on the next run" "$n intents mention it again" test "${n:-1}" = 0
fi

if scenario forget-project "the user forgets one project"; then
  start_daemon
  approve_workspace
  t=$(($(now) - 3600))
  d=$(day_of "$t")
  mkdir -p "$HOME_DIR/src/topsecret/src"
  for i in $(seq 0 5); do
    ev $((t + i * 20)) vscode file_save "{\"path\":\"$HOME_DIR/src/topsecret/src/main.rs\"}" >/dev/null
  done
  run_day "$d"
  project=$(api "$API/intents?date=$d" | jq -r '[.[].tags[] | select(startswith("project:"))][0] // empty' | sed 's/^project://')
  if [[ -z $project ]]; then
    bad "the project is tagged" "no project tag on the day's intents"
  else
    api -X DELETE "$API/v1/memory/project/$project" >/dev/null
    n=$(api "$API/intents/search?q=$project" | jq length)
    check "search finds nothing for a forgotten project" "$n hits" test "${n:-1}" = 0
    tab $((t + 200)) "https://example.org/" "unrelated page"
    run_day "$d"
    n=$(api "$API/intents?date=$d" | jq --arg p "project:$project" '[.[] | select(.tags | index($p))] | length')
    check "a forgotten project does not come back on the next run" "$n intents tagged $project again" test "${n:-1}" = 0
  fi
fi

if scenario retention "retention removes old intents, not only raw events"; then
  start_daemon
  approve_workspace
  t=$(($(now) - 40 * 86400))
  d=$(day_of "$t")
  workday "$t"
  run_day "$d"
  before=$(api "$API/intents?date=$d" | jq length)
  json_put /v1/retention/policy '{"metadata_days":30,"detailed_days":30}'
  api -X POST -H 'content-type: application/json' -d '{"metadata_days":30,"detailed_days":30,"confirm":true}' "$API/v1/retention/purge" >/dev/null
  after=$(api "$API/intents?date=$d" | jq length)
  hits=$(api "$API/intents/search?q=axum" | jq length)
  check "intents older than the policy are purged" "$before intents before, $after after" test "${after:-1}" = 0 -a "${before:-0}" -gt 0
  check "purged intents are not searchable" "$hits search hits for 'axum'" test "${hits:-1}" = 0
fi

if scenario timestamps "events with impossible timestamps"; then
  start_daemon
  s=$(ev $(($(now) * 1000)) firefox tab_change '{"url":"https://ms.example/","title":"ms","tab_id":1}')
  check "a millisecond timestamp is rejected" "status $s" test "$s" = 422
  s=$(ev $(($(now) + 400 * 86400)) firefox tab_change '{"url":"https://future.example/","title":"future","tab_id":1}')
  check "a timestamp a year ahead is rejected" "status $s" test "$s" = 422
  s=$(ev $(($(now) + 30)) firefox tab_change '{"url":"https://skew.example/","title":"skew","tab_id":1}')
  check "small clock skew is accepted" "status $s" test "$s" = 201
fi

if scenario pause "pausing with odd input"; then
  start_daemon
  s=$(code -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' -d '{"paused":true,"until":1}' "$API/v1/capture/pause")
  check "a pause that ends in the past is refused" "status $s" test "$s" = 422
  out=$(ctl pause 307445734561825861m 2>&1)
  paused_for=$(api "$API/v1/status" | jq -r '.capture_paused_until // 0')
  check "an absurd pause length is refused, not wrapped" "intentctl said: ${out:0:120}; paused until $paused_for" \
    test "$(api "$API/v1/status" | jq -r .capture_paused)" = false
  ctl resume >/dev/null 2>&1
  ctl pause 30m >/dev/null 2>&1
  KEEP_HOME=1 start_daemon
  check "a timed pause survives a daemon restart" "not paused after restart" \
    test "$(api "$API/v1/status" | jq -r .capture_paused)" = true
  ctl resume >/dev/null 2>&1
fi

if scenario shell-secrets "secrets typed in a hooked shell"; then
  start_daemon
  secrets=(
    "docker exec db mysql -uroot -phunter2a1"
    "redis-cli -a hunter2b2 ping"
    "docker login -u me -p hunter2c3 registry.example"
    "curl 'https://maps.googleapis.com/maps/api/geocode/json?key=AIzaSyhunter2d4xxxxxxxxxxxxxxxxxxxxxx'"
    "curl https://hooks.slack.com/services/T0000/B0000/hunter2e5XXXXXXXXXXXX"
    "export DB_PASSWORD=hunter2f6"
    "psql postgresql://app:hunter2g7@db.example/app"
  )
  for cmd in "${secrets[@]}"; do ctl send-shell --cmd="$cmd" --cwd=/tmp --exit=0 >/dev/null 2>&1; done
  sleep 0.5
  stored=$(events_json "$(date +%F)" | jq -r '.[] | select(.source=="shell") | .payload.cmd')
  for i in a1 b2 c3 d4 e5 f6 g7; do
    leaked=$(grep -m1 "hunter2$i" <<<"$stored")
    check "secret $i is not stored" "stored: ${leaked:-?}" test -z "$leaked"
  done
  long="echo $(printf 'x%.0s' $(seq 450)) card 4111111111111111 $(printf 'y%.0s' $(seq 300))"
  ctl send-shell --cmd="$long" --cwd=/tmp --exit=0 >/dev/null 2>&1
  sleep 0.3
  n=$(events_json "$(date +%F)" | jq '[.[] | select(.source=="shell") | .payload.cmd | select(test("4111"))] | length')
  check "an over-long command is redacted, not truncated" "$n stored commands still contain the card number" test "${n:-1}" = 0
  before=$(events_json "$(date +%F)" | jq '[.[] | select(.source=="shell")] | length')
  INTENT_PAUSED=0 ctl send-shell --cmd="cargo build" --cwd=/tmp --exit=0 >/dev/null 2>&1
  sleep 0.3
  after=$(events_json "$(date +%F)" | jq '[.[] | select(.source=="shell")] | length')
  check "INTENT_PAUSED=0 does not pause the shell" "$before shell events before, $after after" test "$after" -gt "$before"
fi

if scenario text-secrets "secrets in editor pastes, page excerpts and window titles"; then
  start_daemon
  approve_workspace
  enable_detailed
  t=$(now)
  ev "$t" vscode document_change "{\"path\":\"$HOME_DIR/src/app/.envrc.sh\",\"workspace\":\"$HOME_DIR/src\",\"changes\":[{\"kind\":\"insert\",\"text\":\"AWS_SECRET_ACCESS_KEY=\\\"hunter2h8\\\"\",\"range\":{},\"removed_characters\":0}]}" >/dev/null
  ev "$t" vscode document_change "{\"path\":\"$HOME_DIR/src/app/config.py\",\"workspace\":\"$HOME_DIR/src\",\"changes\":[{\"kind\":\"insert\",\"text\":\"STRIPE_KEY = 'sk_live_hunter2i9xxxxxxxxxxxx'\",\"range\":{},\"removed_characters\":0}]}" >/dev/null
  ev "$t" firefox tab_change "{\"url\":\"https://forum.example/t/1\",\"title\":\"help\",\"tab_id\":1,\"context\":{\"kind\":\"post\",\"text_excerpt\":\"my GITHUB_TOKEN=hunter2j0 does not work\"}}" >/dev/null
  # Keystroke-by-keystroke typing, the way the VS Code companion reports it.
  word="password = hunter2k1"
  for ((i = 0; i < ${#word}; i++)); do
    ch=${word:i:1}
    ev "$t" vscode document_change "{\"path\":\"$HOME_DIR/src/app/db.py\",\"workspace\":\"$HOME_DIR/src\",\"changes\":[{\"kind\":\"insert\",\"text\":\"$ch\",\"range\":{},\"removed_characters\":0}]}" >/dev/null
  done
  ev "$t" linux app_focus '{"app":"firefox","title":"Chase Online Banking — Mozilla Firefox Private Browsing"}' >/dev/null
  sleep 0.5
  all=$(events_json "$(date +%F)")
  for i in h8 i9 j0; do
    n=$(jq --arg s "hunter2$i" '[.. | strings | select(contains($s))] | length' <<<"$all")
    check "secret $i is not stored" "$n stored strings contain it" test "${n:-1}" = 0
  done
  typed=$(jq -r '[.[] | select(.type=="document_change" and .payload.path != null and (.payload.path | endswith("db.py"))) | .payload.changes[]?.text // empty] | join("")' <<<"$all")
  check "a secret typed one key at a time cannot be rebuilt" "the stored keystrokes spell: $typed" test "${typed/hunter2k1/}" = "$typed"
  n=$(jq '[.[] | select(.source=="linux") | .payload.title // empty | select(test("Private Browsing|Incognito"))] | length' <<<"$all")
  check "private-browsing window titles are not stored" "$n stored" test "${n:-1}" = 0
fi

if scenario blocklist "a typo in blocked-domains.yaml"; then
  start_daemon
  mkdir -p "$HOME_DIR/.config/intent"
  printf 'blocked_domains:\n  - https://chase.com\n  - bank.example:443\n' >"$HOME_DIR/.config/intent/blocked-domains.yaml"
  s=$(ev "$(now)" shell command '{"cmd":"ls","cwd":"/","exit_code":0}')
  check "other events are still stored" "status $s for a shell event" test "$s" = 201
fi

if scenario workspace "adding and removing workspaces"; then
  start_daemon
  out=$(ctl workspace add "$HOME_DIR" 2>&1)
  n=$(api "$API/v1/workspaces" | jq --arg h "$HOME_DIR" '[.workspaces[] | select(. == $h)] | length')
  check "the home directory is refused as a workspace" "accepted: $out" test "${n:-1}" = 0
  out=$(ctl workspace add / 2>&1)
  n=$(api "$API/v1/workspaces" | jq '[.workspaces[] | select(. == "/")] | length')
  check "/ is refused as a workspace" "accepted: $out" test "${n:-1}" = 0
  mkdir -p "$HOME_DIR/tmpws"
  ctl workspace add "$HOME_DIR/tmpws" >/dev/null 2>&1
  rm -rf "$HOME_DIR/tmpws"
  (cd "$HOME_DIR" && ctl workspace remove tmpws >/dev/null 2>&1)
  n=$(api "$API/v1/workspaces" | jq --arg p "$HOME_DIR/tmpws" '[.workspaces[] | select(. == $p)] | length')
  check "a deleted workspace can be removed" "it is still listed" test "${n:-1}" = 0
fi

if scenario workspace-recreate "a workspace is deleted and cloned again"; then
  start_daemon INTENT_DISABLE_COLLECTORS=0
  approve_workspace
  mkdir -p "$HOME_DIR/src/proj"
  for _ in $(seq 30); do
    api "$API/v1/status" | jq -e '.collectors[]? | select(.name=="filesystem") | .detail.watched_dirs > 0' >/dev/null 2>&1 && break
    sleep 0.3
  done
  rm -rf "$HOME_DIR/src" && mkdir -p "$HOME_DIR/src/proj" && sleep 1
  echo "fn main() {}" >"$HOME_DIR/src/proj/main.rs"
  for _ in $(seq 40); do
    events_json "$(date +%F)" | jq -e '[.[] | select(.source=="filesystem" and (.payload.path // "" | endswith("main.rs")))] | length > 0' >/dev/null && break
    sleep 0.5
  done
  n=$(events_json "$(date +%F)" | jq '[.[] | select(.source=="filesystem" and (.payload.path // "" | endswith("main.rs")))] | length')
  check "files in the recreated workspace are captured" "$n events after 20 s" test "${n:-0}" -gt 0
fi

if scenario search "odd search input"; then
  start_daemon
  approve_workspace
  t=$(($(now) - 3600))
  workday "$t"
  run_day "$(day_of "$t")"
  for q in '*' 'x' '""' '-' 'NEAR' 'a..b' "$(printf 'é%.0s' $(seq 300))"; do
    enc=$(jq -rn --arg q "$q" '$q|@uri')
    s=$(code -H "Authorization: Bearer $TOKEN" "$API/intents/search?q=$enc")
    check "search for '${q:0:12}' answers 200" "status $s" test "$s" = 200
  done
  n=$(api "$API/intents/search?q=%2A" | jq length)
  check "a search for '*' matches nothing" "$n hits" test "${n:-1}" = 0
  n=$(api "$API/intents/search?q=zzqqxx" | jq length)
  check "a search for an unknown word matches nothing" "$n hits" test "${n:-1}" = 0
fi

if scenario stable-ids "an intent stays openable while the user keeps working"; then
  start_daemon
  approve_workspace
  t=$(($(now) - 3600))
  d=$(day_of "$t")
  workday "$t"
  run_day "$d"
  id=$(api "$API/intents?date=$d" | jq -r '.[0].id // empty')
  ev $((t + 3000)) linux idle_start '{}' >/dev/null
  ev $((t + 3100)) linux idle_end '{}' >/dev/null
  run_day "$d"
  s=$(code -H "Authorization: Bearer $TOKEN" "$API/intents/$id")
  check "an open intent id still resolves after an idle blip" "GET /intents/$id → $s" test "$s" = 200
fi

if scenario forget-race "the user forgets a day while the pipeline runs"; then
  start_daemon
  approve_workspace
  t=$(($(now) - 3600))
  d=$(day_of "$t")
  for i in $(seq 0 400); do tab $((t + i * 5)) "https://race.example/$i" "race page $i"; done
  api -X POST "$API/pipeline/recompute?date=$d" >/dev/null &
  run=$!
  sleep 0.05
  api -X DELETE "$API/v1/memory/date/$d" >/dev/null
  wait "$run"
  n=$(api "$API/intents?date=$d" | jq length)
  check "no intents come back after the forget" "$n intents after the run finished" test "${n:-1}" = 0
fi

if scenario restart "the daemon restarts while events arrive"; then
  start_daemon
  posters=()
  for i in $(seq 0 50); do
    tab $(($(now) - 100 + i)) "https://restart.example/$i" "restart $i" &
    posters+=($!)
  done
  wait "${posters[@]}"
  KEEP_HOME=1 start_daemon
  n=$(events_json "$(date +%F)" | jq '[.[] | select(.payload.url // "" | test("restart.example"))] | length')
  check "every acknowledged event is kept" "$n of 51 stored" test "${n:-0}" = 51
fi

# ---- summary ---------------------------------------------------------------------

printf '\n%d checks passed, %d broken\n' "$pass" "${#broken[@]}"
if ((${#broken[@]})); then
  printf '  %s\n' "${broken[@]}"
  exit 1
fi
