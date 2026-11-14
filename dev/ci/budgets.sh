#!/usr/bin/env bash
# Resource budgets for an installed, running intentd. Fails when any number is over
# budget, so a change that makes the daemon heavier or slower can't land quietly.
#   dev/ci/budgets.sh            # after dev/lima/e2e.sh (needs the release target dir)
#
# Budgets (override with the env var to experiment, not to make a red run green):
#   BUDGET_IDLE_RSS_MB=40        resident memory after the e2e run has settled
#   BUDGET_IDLE_CPU_PCT=1.0      CPU over 30 s of idle
#   BUDGET_INGEST_EPS=1000       events/s, 5000 events from 32 concurrent clients, all 201
#   BUDGET_PIPELINE_MS=1500      forced pipeline run over 2000 events (template labels)
#   BUDGET_PEAK_RSS_MB=80        high-water RSS after ingest + pipeline
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
API=http://127.0.0.1:9477
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
TOKEN=$(cat "$XDG_RUNTIME_DIR/intent/token")
PID=$(pidof intentd)
AUTH=(-H "Authorization: Bearer $TOKEN")

IDLE_RSS_MB=${BUDGET_IDLE_RSS_MB:-40}
IDLE_CPU_PCT=${BUDGET_IDLE_CPU_PCT:-1.0}
INGEST_EPS=${BUDGET_INGEST_EPS:-1000}
PIPELINE_MS=${BUDGET_PIPELINE_MS:-1500}
PEAK_RSS_MB=${BUDGET_PEAK_RSS_MB:-80}

failures=0
report() { # name value unit budget op(le|ge)
  local ok
  ok=$(awk -v v="$2" -v b="$4" -v op="$5" 'BEGIN { print (op == "le" ? v <= b : v >= b) ? 1 : 0 }')
  if [[ $ok == 1 ]]; then
    printf '  ok    %-22s %10s %-4s (budget %s %s)\n' "$1" "$2" "$3" "${5/le/<=}" "$4"
  else
    printf '  OVER  %-22s %10s %-4s (budget %s %s)\n' "$1" "$2" "$3" "${5/le/<=}" "$4"
    failures=$((failures + 1))
  fi
  [[ -n "${GITHUB_STEP_SUMMARY:-}" ]] &&
    echo "| $1 | $2 $3 | ${5/le/≤}${5/ge/≥} $4 | $([[ $ok == 1 ]] && echo ok || echo '**over**') |" >>"$GITHUB_STEP_SUMMARY"
  return 0
}
kb() { awk -v k="$1" '$1 == k":" { print $2 }' /proc/"$PID"/status; }
mb() { awk -v k="$1" 'BEGIN { printf "%.1f", k / 1024 }'; }
cpu_ticks() { awk '{ print $14 + $15 }' /proc/"$PID"/stat; }

[[ -n "${GITHUB_STEP_SUMMARY:-}" ]] && printf '### intentd budgets (%s)\n\n| measure | value | budget | |\n|---|---|---|---|\n' "$(uname -m)" >>"$GITHUB_STEP_SUMMARY"

echo "== build the load generator"
(cd "$ROOT" && cargo build --release --locked -q -p intentd --example ingest_load)
LOAD="$TARGET_DIR/release/examples/ingest_load"

echo "== idle (settle 15 s, then measure 30 s)"
sleep 15
report "idle RSS" "$(mb "$(kb VmRSS)")" MB "$IDLE_RSS_MB" le
hz=$(getconf CLK_TCK)
t0=$(cpu_ticks)
sleep 30
t1=$(cpu_ticks)
report "idle CPU" "$(awk -v a="$t0" -v b="$t1" -v hz="$hz" 'BEGIN { printf "%.2f", (b - a) / hz / 30 * 100 }')" % "$IDLE_CPU_PCT" le

echo "== ingest"
out=$("$LOAD" --url "$API" --n 5000 --concurrency 32)
echo "  $out"
report "ingest rate" "$(echo "$out" | jq '.events_per_second')" ev/s "$INGEST_EPS" ge
bad=$(echo "$out" | jq '[.status_codes | to_entries[] | select(.key != "201") | .value] | add // 0')
report "non-201 responses" "$bad" "" 0 le

echo "== pipeline (2000 events on a past day, forced)"
day=$(date -d '3 days ago' +%F)
"$LOAD" --url "$API" --n 2000 --concurrency 16 --date "$day" >/dev/null
best=
for _ in 1 2 3; do
  start=$(date +%s%N)
  curl -fsS -X POST "${AUTH[@]}" "$API/pipeline/recompute?date=$day" >/dev/null
  ms=$((($(date +%s%N) - start) / 1000000))
  [[ -z $best || $ms -lt $best ]] && best=$ms
done
report "pipeline 2000 events" "$best" ms "$PIPELINE_MS" le
report "peak RSS" "$(mb "$(kb VmHWM)")" MB "$PEAK_RSS_MB" le

if ((failures > 0)); then
  echo "BUDGETS FAILED: $failures over budget" >&2
  exit 1
fi
echo "BUDGETS OK"
