"""Append-only, privacy-safe JSONL diagnostics for the Role B pipeline."""

from __future__ import annotations

import json
import re
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


_SAFE_KEYS = {
    "timestamp", "type", "event_id", "error", "severity", "date", "source_hash",
    "status", "event_count", "duration_ms", "warning_count",
    "semantic_provider_identity", "semantic_fallback_reason", "intent_count", "cached", "outcome",
}
_SENSITIVE_ERROR = re.compile(
    r"(?:"
    r"\b(?:password|passwd|secret|api[_-]?key|access[_-]?key|token|authorization|bearer)\b"
    r"|-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----"
    r"|\b(?:gh[pous]_[A-Za-z0-9_]+|xox[baprs]-[A-Za-z0-9-]+|eyJ[A-Za-z0-9_-]+\.)"
    r"|\b(?:https?|chrome|about)://|\[redacted\]|<redacted>"
    r"|[\[{]\s*[\"']?(?:raw|payload|text|content|document|url)[\"']?\s*:"
    r")",
    re.IGNORECASE,
)


def log_line_safe(data: dict[str, Any]) -> str:
    """Serialize an allowlisted diagnostic record without sensitive event data."""

    safe_data = {key: value for key, value in data.items() if key in _SAFE_KEYS}
    error = safe_data.get("error")
    if isinstance(error, str) and _SENSITIVE_ERROR.search(error):
        safe_data["error"] = "<redacted>"
    return json.dumps(safe_data, ensure_ascii=False, separators=(",", ":"))


class DiagnosticsLogger:
    """Immediately persisted local diagnostics with a small pending-write buffer."""

    def __init__(self, log_file: str = "role-b.jsonl") -> None:
        self.log_file = Path(log_file)
        self.buffer: list[str] = []

    def log_event_validation_error(self, event_id: str, error: str, severity: str = "warning") -> None:
        self._append({
            "timestamp": self._timestamp(),
            "type": "event_validation_error",
            "event_id": event_id,
            "error": error,
            "severity": severity,
        })

    def log_pipeline_run(
        self,
        date: str,
        source_hash: str,
        status: str,
        event_count: int,
        duration_ms: int,
        warning_count: int = 0,
        semantic_provider_identity: str | None = None,
        semantic_fallback_reason: str | None = None,
    ) -> None:
        self._append({
            "timestamp": self._timestamp(),
            "type": "pipeline_run",
            "date": date,
            "source_hash": source_hash[:16],
            "status": status,
            "event_count": event_count,
            "duration_ms": duration_ms,
            "warning_count": warning_count,
            "semantic_provider_identity": semantic_provider_identity,
            "semantic_fallback_reason": semantic_fallback_reason,
        })

    def log_cache_hit(self, date: str, source_hash: str) -> None:
        self._append({
            "timestamp": self._timestamp(),
            "type": "cache_hit",
            "date": date,
            "source_hash": source_hash[:16],
        })

    def log_scheduled_ingest(
        self,
        *,
        date: str,
        event_count: int,
        intent_count: int,
        cached: bool,
        duration_ms: int,
        outcome: str,
    ) -> None:
        """Record only aggregate scheduler diagnostics and a fixed outcome class."""

        self._append({
            "timestamp": self._timestamp(),
            "type": "scheduled_ingest",
            "date": date,
            "event_count": event_count,
            "intent_count": intent_count,
            "cached": cached,
            "duration_ms": duration_ms,
            "outcome": outcome,
        })

    def flush(self) -> None:
        """Append all pending JSON lines and clear only after a successful write."""

        if not self.buffer:
            return
        self.log_file.parent.mkdir(parents=True, exist_ok=True)
        with self.log_file.open("a", encoding="utf-8", newline="\n") as output:
            output.write("\n".join(self.buffer) + "\n")
        self.buffer.clear()

    def _append(self, record: dict[str, Any]) -> None:
        self.buffer.append(log_line_safe(record))
        self.flush()

    @staticmethod
    def _timestamp() -> str:
        return datetime.now(timezone.utc).isoformat()
