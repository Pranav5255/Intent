"""Per-user approved workspace configuration for the fallback watcher."""

from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path


MAX_WORKSPACES = 5


def config_path(home: Path | None = None) -> Path:
    home = home or Path.home()
    config_home = Path(os.environ.get("XDG_CONFIG_HOME", home / ".config"))
    return config_home / "intent-os" / "config.json"


def load(path: Path | None = None) -> dict[str, list[str]]:
    path = path or config_path()
    if not path.exists():
        return {"workspaces": []}
    loaded = json.loads(path.read_text(encoding="utf-8"))
    workspaces = loaded.get("workspaces", [])
    if not isinstance(workspaces, list) or not all(isinstance(item, str) for item in workspaces):
        raise ValueError(f"invalid workspace config: {path}")
    return {"workspaces": workspaces}


def save(config: dict[str, list[str]], path: Path | None = None) -> Path:
    path = path or config_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile("w", encoding="utf-8", dir=path.parent, delete=False) as temporary:
        json.dump(config, temporary, indent=2)
        temporary.write("\n")
        temporary_path = Path(temporary.name)
    temporary_path.replace(path)
    return path


def validate_workspace(path: Path, home: Path | None = None) -> Path:
    home = (home or Path.home()).resolve()
    candidate = path.expanduser().resolve()
    if not candidate.is_dir():
        raise ValueError(f"workspace is not a directory: {path}")
    if candidate == home:
        raise ValueError("the entire home directory cannot be a workspace")
    try:
        candidate.relative_to(home)
    except ValueError as exc:
        raise ValueError("workspace must be inside the user home directory") from exc
    return candidate


def add(path: Path, config: dict[str, list[str]], home: Path | None = None) -> dict[str, list[str]]:
    candidate = str(validate_workspace(path, home))
    existing = config["workspaces"]
    if candidate not in existing and len(existing) >= MAX_WORKSPACES:
        raise ValueError(f"at most {MAX_WORKSPACES} workspaces may be watched")
    return {"workspaces": [*existing, candidate] if candidate not in existing else existing}


def remove(path: Path, config: dict[str, list[str]]) -> dict[str, list[str]]:
    candidate = str(path.expanduser().resolve())
    return {"workspaces": [item for item in config["workspaces"] if item != candidate]}
