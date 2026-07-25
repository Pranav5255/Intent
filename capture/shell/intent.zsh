# Intent shell capture for zsh.
# Sourced by intentctl shell enable.

_intent_endpoint="${INTENT_EVENT_ENDPOINT:-http://127.0.0.1:9477/v1/event}"
_intent_last_cmd=""

_intent_emit_command() {
  local cmd="$1"
  local exit_code="$2"
  [[ -z "$cmd" ]] && return 0
  (
    INTENT_CMD="$cmd" INTENT_CWD="$PWD" INTENT_EXIT="$exit_code" INTENT_ENDPOINT="$_intent_endpoint" \
      python3 - <<'PY'
import json
import os
import sys
import time
import uuid
from urllib import request

cmd = os.environ.get("INTENT_CMD", "")
cwd = os.environ.get("INTENT_CWD", "")
endpoint = os.environ.get("INTENT_ENDPOINT", "http://127.0.0.1:9477/v1/event")
try:
    exit_code = int(os.environ.get("INTENT_EXIT", "0"))
except ValueError:
    exit_code = 0
if not cmd:
    sys.exit(0)
event = {
    "id": str(uuid.uuid4()),
    "schema_version": 1,
    "ts": int(time.time()),
    "source": "shell",
    "type": "command",
    "payload": {
        "cmd": cmd[:500],
        "cwd": cwd[:4096],
        "exit_code": exit_code,
    },
}
try:
    body = json.dumps(event).encode("utf-8")
    req = request.Request(
        endpoint,
        data=body,
        method="POST",
        headers={"Content-Type": "application/json"},
    )
    request.urlopen(req, timeout=1)
except OSError:
    pass
PY
  ) >/dev/null 2>&1 &
}

_intent_preexec() {
  _intent_last_cmd="$1"
}

_intent_precmd() {
  [[ -z "$_intent_last_cmd" ]] && return 0
  _intent_emit_command "$_intent_last_cmd" "$?"
  _intent_last_cmd=""
}

if [[ -z "${INTENT_ZSH_HOOKS_INSTALLED:-}" ]]; then
  typeset -ga preexec_functions
  typeset -ga precmd_functions
  preexec_functions+=(_intent_preexec)
  precmd_functions+=(_intent_precmd)
  export INTENT_ZSH_HOOKS_INSTALLED=1
fi
