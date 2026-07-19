"""Approved workspace paths for editor and filesystem capture consent."""

from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path


def config_path(home: Path | None = None) -> Path:
    override = os.environ.get("INTENT_OS_WORKSPACES_CONFIG")
    if override:
        return Path(override).expanduser()
    home = home or Path.home()
    config_home = Path(os.environ.get("XDG_CONFIG_HOME", home / ".config"))
    return config_home / "intent-os" / "workspaces.json"


def _normalise(raw: object) -> dict[str, list[str]]:
    if not isinstance(raw, dict):
        raise ValueError("workspaces config must be an object")
    entries = raw.get("workspaces", [])
    if not isinstance(entries, list):
        raise ValueError("workspaces must be a list")
    resolved: list[str] = []
    seen: set[str] = set()
    for item in entries:
        if not isinstance(item, str) or not item.strip():
            raise ValueError("workspace paths must be non-empty strings")
        path = str(Path(item).expanduser().resolve())
        if path not in seen:
            seen.add(path)
            resolved.append(path)
    return {"workspaces": resolved}


def load(path: Path | None = None) -> dict[str, list[str]]:
    path = path or config_path()
    if not path.exists():
        return {"workspaces": []}
    try:
        return _normalise(json.loads(path.read_text(encoding="utf-8")))
    except json.JSONDecodeError as exc:
        raise ValueError(f"invalid workspaces config: {path}") from exc


def save(config: dict[str, list[str]], path: Path | None = None) -> Path:
    path = path or config_path()
    normalised = _normalise(config)
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile("w", encoding="utf-8", dir=path.parent, delete=False) as temporary:
        json.dump(normalised, temporary, indent=2)
        temporary.write("\n")
        temporary_path = Path(temporary.name)
    temporary_path.replace(path)
    return path


def add(path: Path | str, config: dict[str, list[str]] | None = None) -> dict[str, list[str]]:
    resolved = str(Path(path).expanduser().resolve())
    current = _normalise(config) if config is not None else load()
    if resolved not in current["workspaces"]:
        current["workspaces"].append(resolved)
    return current


def remove(path: Path | str, config: dict[str, list[str]] | None = None) -> dict[str, list[str]]:
    resolved = str(Path(path).expanduser().resolve())
    current = _normalise(config) if config is not None else load()
    current["workspaces"] = [item for item in current["workspaces"] if item != resolved]
    return current
