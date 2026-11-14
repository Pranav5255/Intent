#!/usr/bin/env bash
# Resource and latency measurements for an installed, running intentd (Lima VM):
#   dev/lima/measure.sh idle            # RSS/PSS + CPU% over 60 s of idle
#   dev/lima/measure.sh ingest [N] [C]  # events/s via examples/ingest_load (N events, C concurrent)
#   dev/lima/measure.sh pipeline [N]    # seed N events on a past day, time a forced pipeline run
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
API=http://127.0.0.1:9477
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
TOKEN=$(cat "$XDG_RUNTIME_DIR/intent/token")
PID=$(pidof intentd)

mem() {
  echo "pid $PID, uptime $(ps -o etimes= -p "$PID" | tr -d ' ') s, threads $(awk '/^Threads/{print $2}' /proc/"$PID"/status)"
  grep -E '^(VmRSS|VmHWM|RssAnon|RssFile)' /proc/"$PID"/status | tr -s ' '
  grep -E '^(Pss|Pss_Anon|Pss_File)' /proc/"$PID"/smaps_rollup | tr -s ' '
}

cpu_ticks() { awk '{print $14 + $15}' /proc/"$PID"/stat; }

build_load() {
  (cd "$ROOT" && cargo build --release -q -p intentd --example ingest_load)
}

case "${1:-idle}" in
  idle)
    mem
    hz=$(getconf CLK_TCK)
    t0=$(cpu_ticks); w0=$(cat /proc/"$PID"/task/*/status | awk '/^voluntary_ctxt_switches/{s+=$2} END{print s}')
    sleep 60
    t1=$(cpu_ticks); w1=$(cat /proc/"$PID"/task/*/status | awk '/^voluntary_ctxt_switches/{s+=$2} END{print s}')
    awk -v a="$t0" -v b="$t1" -v hz="$hz" 'BEGIN { printf "cpu over 60 s idle: %d ticks = %.3f%%\n", b - a, (b - a) / hz / 60 * 100 }'
    echo "wake-ups (voluntary context switches) over 60 s: $((w1 - w0))"
    ;;
  ingest)
    build_load
    before=$(cpu_ticks)
    "$TARGET_DIR/release/examples/ingest_load" --url "$API" --n "${2:-5000}" --concurrency "${3:-32}"
    after=$(cpu_ticks)
    echo "intentd cpu during load: $((after - before)) ticks"
    curl -fsS -H "Authorization: Bearer $TOKEN" "$API/v1/status" |
      jq -c '.ingestion | {accepted, inserted, batches, queue_high_watermark, average_queue_wait_ms, average_write_ms, dropped_by_reason}'
    mem
    ;;
  pipeline)
    build_load
    day=$(date -d '3 days ago' +%F)
    "$TARGET_DIR/release/examples/ingest_load" --url "$API" --n "${2:-2000}" --concurrency 16 --date "$day"
    curl -fsS -H "Authorization: Bearer $TOKEN" "$API/v1/export/day/meta?date=$day" | jq -c '{date, event_count}'
    for run in 1 2; do
      start=$(date +%s%N)
      out=$(curl -fsS -X POST -H "Authorization: Bearer $TOKEN" "$API/pipeline/recompute?date=$day")
      end=$(date +%s%N)
      echo "forced run $run: $(( (end - start) / 1000000 )) ms, $(echo "$out" | jq '.intents | length') root intents, $(echo "$out" | jq '[.intents[] | .. | objects | select(has("label"))] | length') nodes, warnings $(echo "$out" | jq '.warnings | length')"
    done
    start=$(date +%s%N)
    out=$(curl -fsS -X POST -H "Authorization: Bearer $TOKEN" "$API/pipeline/run?date=$day")
    end=$(date +%s%N)
    echo "cached run: $(( (end - start) / 1000000 )) ms (cached=$(echo "$out" | jq .cached))"
    curl -fsS -H "Authorization: Bearer $TOKEN" "$API/v1/status" | jq -c '.pipeline.last_run'
    echo "$out" | jq -r '.intents[] | "  \(.label) — \(.summary)"' | head -20
    mem
    ;;
  labels)
    # Pipeline with the local provider over the engine's sample days, vs templates.
    model=${2:-qwen2.5:3b}
    put() { curl -fsS -X PUT -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' -d "$1" "$API/settings/llm"; }
    for mode in ${MODES:-template local}; do
      if [[ $mode == template ]]; then
        put '{"provider":"none"}' | jq -c '{provider}'
      else
        put "{\"provider\":\"local\",\"model\":\"$model\",\"embedding_model\":\"nomic-embed-text\",\"enable_semantic_cluster\":true}" |
          jq -c '{provider, model, embedding_model, semantic_cluster_enabled, local_server_reachable}'
      fi
      for f in sample-day coding-research-day debugging-day interrupted-day; do
        start=$(date +%s%N)
        out=$(curl -fsS -X POST -H "Authorization: Bearer $TOKEN" -H 'content-type: application/json' \
          --data-binary @"$ROOT/crates/intent-engine/tests/fixtures/$f.json" "$API/pipeline/run-replay")
        end=$(date +%s%N)
        echo "[$mode] $f: $(( (end - start) / 1000000 )) ms, $(echo "$out" | jq '.intents|length') roots, warnings $(echo "$out" | jq -c '[.warnings[].message]')"
        echo "$out" | jq -r '.intents[] | .. | objects | select(has("label") and has("depth")) | "   \("  " * .depth)\(.label) — \(.summary) (conf \(.confidence))"'
      done
    done
    curl -fsS -H "Authorization: Bearer $TOKEN" "$API/v1/status" | jq -c '.llm | {provider, reachable, semantic_enabled, harness, embedder}'
    ;;
  eval)
    # Real local-model correlation eval (needs Ollama with the model pulled).
    (cd "$ROOT" && cargo build --release -q -p intent-llm --bin intent-eval)
    shift
    time "$TARGET_DIR/release/intent-eval" --provider local --model "${1:-qwen2.5:3b}" "${@:2}"
    ;;
  *) echo "usage: $0 idle|ingest [N] [C]|pipeline [N]|eval [MODEL] [intent-eval flags]" >&2; exit 2 ;;
esac
