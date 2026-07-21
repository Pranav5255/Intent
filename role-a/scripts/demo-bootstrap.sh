#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROLE_A="$ROOT"
ROLE_B="$(cd "$ROOT/../role-b" && pwd)"
DEMO_APP="$HOME/projects/taskflow-app"
PYTHON="${PYTHON:-python3}"

echo "== Intent demo bootstrap =="

mkdir -p "$DEMO_APP/src"
if [[ ! -f "$DEMO_APP/src/auth.tsx" ]]; then
  cat > "$DEMO_APP/src/auth.tsx" <<'EOF'
export async function login(email: string, password: string) {
  // TODO show invalid credentials message
  const request = { method: "POST", body: JSON.stringify({ email, password }) };
  await fetch("/api/login", request);
}
EOF
  echo "Created $DEMO_APP/src/auth.tsx"
fi

cd "$ROLE_A"
export INTENT_OS_INSTALL_ROOT="$ROLE_A"
"$PYTHON" tools/intent_osctl.py enable || true
"$PYTHON" tools/intent_osctl.py workspace add "$DEMO_APP"
"$PYTHON" tools/intent_osctl.py detailed editor enable || true
"$PYTHON" tools/intent_osctl.py shell enable --shell "${SHELL##*/}" || true
if command -v code >/dev/null 2>&1; then
  "$PYTHON" tools/intent_osctl.py vscode install || true
elif command -v cursor >/dev/null 2>&1; then
  "$PYTHON" tools/intent_osctl.py cursor install || true
fi

echo
echo "Role A health:  http://127.0.0.1:9477/v1/status"
echo "Role A export:  http://127.0.0.1:9477/v1/export/day?date=2026-07-13"
echo
echo "Start Role B from $ROLE_B:"
echo "  cd $ROLE_B && .venv/bin/uvicorn intent_engine.api:app --host 127.0.0.1 --port 9478"
echo
echo "Seed Role B replay fixture:"
echo "  curl -s -X POST http://127.0.0.1:9478/pipeline/run-replay -H 'Content-Type: application/json' --data-binary @$ROLE_B/tests/fixtures/demo-day.json"
echo
echo "Fallback replay into Role A:"
echo "  $PYTHON scripts/emit_fixture.py fixtures/demo-day.json"
