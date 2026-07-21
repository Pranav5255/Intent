#!/usr/bin/env bash
set -euo pipefail

RUNTIME="${1:?usage: scripts/stop-video-demo.sh /tmp/intent-os-video-demo-...}"

for name in electron renderer role-b role-a; do
  pid_file="$RUNTIME/$name.pid"
  if [[ -f "$pid_file" ]]; then
    pid="$(cat "$pid_file")"
    if kill -0 "$pid" 2>/dev/null; then
      kill "$pid"
    fi
  fi
done

printf 'Stopped demo processes recorded in %s\n' "$RUNTIME"
