#!/usr/bin/env python3
"""Best-effort, redacting sender used by Intent bash and zsh hooks."""

from __future__ import annotations

import json
import os
import re
import sys
import time
import uuid
from urllib import request


SENSITIVE_COMMAND = re.compile(
    r"(?:password|passwd|secret|api[_-]?key|access[_-]?key|token|bearer|ssh\b|gpg\b|sudo\b)",
    re.IGNORECASE,
)
MAX_COMMAND_LENGTH = 500


def safe_command(command: str) -> str:
    command = command.strip()
    if not command:
        return ""
    if len(command) > MAX_COMMAND_LENGTH or SENSITIVE_COMMAND.search(command):
        return "<redacted>"
    return command


def parse_nul_payload(raw: bytes) -> tuple[str, str, int, int]:
    fields = raw.decode("utf-8", errors="replace").split("\0")
    if len(fields) < 4:
        raise ValueError("expected command, cwd, exit code and duration")
    command, cwd, exit_code, duration_ms = fields[:4]
    return command, cwd, int(exit_code), max(0, int(duration_ms))


def event_from_nul(raw: bytes) -> dict[str, object]:
    command, cwd, exit_code, duration_ms = parse_nul_payload(raw)
    return {
        "id": str(uuid.uuid4()),
        "ts": int(time.time()),
        "source": "shell",
        "type": "command",
        "payload": {
            "cmd": safe_command(command),
            "cwd": cwd,
            "exit_code": exit_code,
            "duration_ms": duration_ms,
        },
    }


def send(endpoint: str, payload: dict[str, object]) -> None:
    body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
    req = request.Request(endpoint, data=body, method="POST", headers={"Content-Type": "application/json"})
    with request.urlopen(req, timeout=0.2):
        pass


def main() -> int:
    try:
        payload = event_from_nul(sys.stdin.buffer.read())
        if payload["payload"]["cmd"]:
            send(os.environ.get("INTENT_EVENT_ENDPOINT", "http://127.0.0.1:9477/v1/event"), payload)
    except Exception:
        # Shell instrumentation must never change a user command's exit result.
        pass
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
