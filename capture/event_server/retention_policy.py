"""Persisted local retention windows for Role A event storage."""

from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path
from typing import Any

DEFAULT_POLICY: dict[str, Any] = {
    "metadata_days": None,
    "detailed_days": None,
}


def config_path(home: Path | None = None) -> Path:
    override = os.environ.get("INTENT_RETENTION_POLICY_CONFIG")
    if override:
        return Path(override).expanduser()
    home = home or Path.home()
    config_home = Path(os.environ.get("XDG_CONFIG_HOME", home / ".config"))
    return config_home / "intent" / "retention-policy.json"


def _normalise(raw: object) -> dict[str, Any]:
    if not isinstance(raw, dict):
        raise ValueError("retention policy must be an object")
    policy = DEFAULT_POLICY.copy()
    for key in ("metadata_days", "detailed_days"):
        value = raw.get(key)
        if value is None:
            policy[key] = None
            continue
        if isinstance(value, bool) or not isinstance(value, int) or value < 1 or value > 36_500:
            raise ValueError(f"{key} must be a positive integer or null")
        policy[key] = value
    return policy


def load(path: Path | None = None) -> dict[str, Any]:
    path = path or config_path()
    if not path.exists():
        return DEFAULT_POLICY.copy()
    try:
        return _normalise(json.loads(path.read_text(encoding="utf-8")))
    except json.JSONDecodeError as exc:
        raise ValueError(f"invalid retention policy: {path}") from exc


def save(policy: dict[str, Any], path: Path | None = None) -> Path:
    path = path or config_path()
    normalised = _normalise(policy)
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile("w", encoding="utf-8", dir=path.parent, delete=False) as temporary:
        json.dump(normalised, temporary, indent=2)
        temporary.write("\n")
        temporary_path = Path(temporary.name)
    temporary_path.replace(path)
    return path


def public_summary(policy: dict[str, Any]) -> str:
    metadata_days = policy.get("metadata_days")
    detailed_days = policy.get("detailed_days")
    if metadata_days is None and detailed_days is None:
        return "indefinite"
    parts: list[str] = []
    if isinstance(metadata_days, int):
        parts.append(f"metadata:{metadata_days}d")
    if isinstance(detailed_days, int):
        parts.append(f"detailed:{detailed_days}d")
    return ", ".join(parts) if parts else "indefinite"
