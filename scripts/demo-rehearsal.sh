#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROLE_A="$ROOT/role-a"
ROLE_B="$ROOT/role-b"
FIXTURE_DATE="2026-07-13"
PYTHON="${PYTHON:-python3}"
FAILURES=0

pass() { echo "PASS: $1"; }
fail() { echo "FAIL: $1"; FAILURES=$((FAILURES + 1)); }

check_json() {
  local url="$1"
  if curl -sf "$url" >/dev/null; then
    pass "$url"
  else
    fail "$url"
  fi
}

echo "== Intent OS demo rehearsal =="

echo
echo "-- Role B unit tests --"
if (cd "$ROLE_B" && .venv/bin/pytest -q tests/test_digest.py tests/test_context.py tests/test_pipeline.py); then
  pass "Role B golden tests"
else
  fail "Role B golden tests"
fi

echo
echo "-- Fallback replay path --"
if curl -sf http://127.0.0.1:9477/healthz >/dev/null 2>&1; then
  "$PYTHON" "$ROLE_A/scripts/emit_fixture.py" "$ROLE_A/fixtures/demo-day.json" || fail "emit_fixture"
  pass "emit_fixture into Role A"
else
  echo "SKIP: Role A not running for emit_fixture"
fi

if curl -sf http://127.0.0.1:9478/healthz >/dev/null 2>&1; then
  curl -sf -X POST "http://127.0.0.1:9478/pipeline/run-replay" \
    -H "Content-Type: application/json" \
    --data-binary @"$ROLE_B/tests/fixtures/demo-day.json" >/dev/null \
    && pass "pipeline run-replay" || fail "pipeline run-replay"

  DIGEST="$(curl -sf "http://127.0.0.1:9478/intents/digest?date=$FIXTURE_DATE")"
  if echo "$DIGEST" | grep -q "Building Login Feature"; then
    pass "digest headline"
  else
    fail "digest headline"
  fi

  PARENT_ID="$(echo "$DIGEST" | "$PYTHON" -c 'import json,sys; print(json.load(sys.stdin)["top_intent_ids"][0])')"
  CONTEXT="$(curl -sf "http://127.0.0.1:9478/intents/$PARENT_ID/context")"
  if echo "$CONTEXT" | grep -qi "auth.tsx" && echo "$CONTEXT" | grep -qi "npm"; then
    pass "intent context markdown"
  else
    fail "intent context markdown"
  fi
else
  echo "SKIP: Role B not running for HTTP checks"
fi

echo
echo "-- Live capture health (optional) --"
if STATUS="$(curl -sf http://127.0.0.1:9477/v1/status 2>/dev/null)"; then
  for source in vscode firefox shell linux; do
    if echo "$STATUS" | grep -q "\"$source\""; then
      pass "Role A status includes $source"
    else
      fail "Role A status includes $source"
    fi
  done
else
  echo "SKIP: Role A status unavailable"
fi

echo
if [[ "$FAILURES" -eq 0 ]]; then
  echo "Rehearsal gate: READY for Role C"
  exit 0
fi

echo "Rehearsal gate: $FAILURES failure(s)"
exit 1
