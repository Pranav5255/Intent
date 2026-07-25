"""Configuration for broad, event-driven filesystem observation."""

from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path

DEFAULT = {"all_accessible": False}


def config_path(home: Path | None = None) -> Path:
    home = home or Path.home()
    return Path(os.environ.get("XDG_CONFIG_HOME", home / ".config")) / "intent" / "filesystem-capture.json"


def load(path: Path | None = None) -> dict[str, bool]:
    path = path or config_path()
    if not path.exists():
        return DEFAULT.copy()
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict) or not isinstance(value.get("all_accessible", False), bool):
        raise ValueError(f"invalid filesystem capture configuration: {path}")
    return {"all_accessible": value.get("all_accessible", False)}


def save(config: dict[str, bool], path: Path | None = None) -> Path:
    path = path or config_path()
    value = {"all_accessible": bool(config.get("all_accessible", False))}
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile("w", encoding="utf-8", dir=path.parent, delete=False) as temporary:
        json.dump(value, temporary, indent=2)
        temporary.write("\n")
        temporary_path = Path(temporary.name)
    temporary_path.replace(path)
    return path
