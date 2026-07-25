"""Small JSONL logger shared by Role A background processes."""

from __future__ import annotations

import json
import logging
import os
from datetime import datetime, timezone
from pathlib import Path


class JsonlFormatter(logging.Formatter):
    """Render stable, grep- and jq-friendly local diagnostic records."""

    def format(self, record: logging.LogRecord) -> str:
        data: dict[str, object] = {
            "ts": datetime.fromtimestamp(record.created, timezone.utc).isoformat().replace("+00:00", "Z"),
            "level": record.levelname.lower(),
            "component": getattr(record, "component", record.name),
            "event": getattr(record, "event", record.getMessage()),
        }
        for field in ("source", "type", "id", "error_type", "detail"):
            value = getattr(record, field, None)
            if value is not None:
                data[field] = value
        return json.dumps(data, separators=(",", ":"), ensure_ascii=False)


def log_directory() -> Path:
    data_home = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local" / "share"))
    return data_home / "intent" / "logs"


def configure_jsonl_logger(component: str, filename: str) -> logging.Logger:
    """Return an idempotently configured local file logger."""
    logger = logging.getLogger(f"intent.{component}")
    logger.setLevel(logging.INFO)
    logger.propagate = False
    path = log_directory() / filename
    path.parent.mkdir(parents=True, exist_ok=True)
    if not any(isinstance(handler, logging.FileHandler) and Path(handler.baseFilename) == path for handler in logger.handlers):
        handler = logging.FileHandler(path, encoding="utf-8")
        handler.setFormatter(JsonlFormatter())
        logger.addHandler(handler)
    return logger
