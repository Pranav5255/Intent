# Intent shell capture for zsh.
# Sourced by intent-osctl shell enable.

_intent_os_endpoint="${INTENT_OS_EVENT_ENDPOINT:-http://127.0.0.1:9477/v1/event}"
_intent_os_last_cmd=""

_intent_os_emit_command() {
  local cmd="$1"
  local exit_code="$2"
  [[ -z "$cmd" ]] && return 0
  (
    INTENT_OS_CMD="$cmd" INTENT_OS_CWD="$PWD" INTENT_OS_EXIT="$exit_code" INTENT_OS_ENDPOINT="$_intent_os_endpoint" \
      python3 - <<'PY'
import json
import os
import sys
import time
import uuid
from urllib import request

cmd = os.environ.get("INTENT_OS_CMD", "")
cwd = os.environ.get("INTENT_OS_CWD", "")
endpoint = os.environ.get("INTENT_OS_ENDPOINT", "http://127.0.0.1:9477/v1/event")
try:
    exit_code = int(os.environ.get("INTENT_OS_EXIT", "0"))
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

_intent_os_preexec() {
  _intent_os_last_cmd="$1"
}

_intent_os_precmd() {
  [[ -z "$_intent_os_last_cmd" ]] && return 0
  _intent_os_emit_command "$_intent_os_last_cmd" "$?"
  _intent_os_last_cmd=""
}

if [[ -z "${INTENT_OS_ZSH_HOOKS_INSTALLED:-}" ]]; then
  typeset -ga preexec_functions
  typeset -ga precmd_functions
  preexec_functions+=(_intent_os_preexec)
  precmd_functions+=(_intent_os_precmd)
  export INTENT_OS_ZSH_HOOKS_INSTALLED=1
fi
