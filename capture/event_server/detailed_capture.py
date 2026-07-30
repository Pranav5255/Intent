"""Consent, configuration and classification for detailed-capture events."""

from __future__ import annotations

import json
import os
import tempfile
from fnmatch import fnmatch
from copy import deepcopy
from pathlib import Path
from typing import Any

from tools.workspaces import load as load_workspaces


DETAILED_EVENT_KINDS = {("vscode", "document_change"), ("firefox", "user_action"), ("filesystem", "file_content")}
DEFAULT_EXCLUDED_PATTERNS = [
    ".env",
    ".env.*",
    "*.pem",
    "*.key",
    "id_rsa*",
    "*secret*",
    "*credential*",
    "*password*",
    "*token*",
]
DEFAULT_CONFIG: dict[str, Any] = {
    "editor": {"enabled": False, "excluded_patterns": DEFAULT_EXCLUDED_PATTERNS},
    "browser": {"enabled": False, "context_enabled": False},
    "filesystem": {"enabled": False},
}


def config_path(home: Path | None = None) -> Path:
    """Return the per-user detailed-capture consent configuration path."""
    override = os.environ.get("INTENT_DETAILED_CAPTURE_CONFIG")
    if override:
        return Path(override).expanduser()
    home = home or Path.home()
    config_home = Path(os.environ.get("XDG_CONFIG_HOME", home / ".config"))
    return config_home / "intent" / "detailed-capture.json"


def _normalise(raw: object) -> dict[str, Any]:
    if not isinstance(raw, dict):
        raise ValueError("detailed-capture config must be an object")
    config = deepcopy(DEFAULT_CONFIG)
    for source in ("editor", "browser", "filesystem"):
        value = raw.get(source, {})
        if not isinstance(value, dict):
            raise ValueError(f"detailed-capture {source} config must be an object")
        if "enabled" in value and not isinstance(value["enabled"], bool):
            raise ValueError(f"detailed-capture {source}.enabled must be a boolean")
        config[source]["enabled"] = value.get("enabled", config[source]["enabled"])
    browser_context = raw.get("browser", {}).get("context_enabled", False)
    if not isinstance(browser_context, bool):
        raise ValueError("detailed-capture browser.context_enabled must be a boolean")
    config["browser"]["context_enabled"] = browser_context

    patterns = raw.get("editor", {}).get("excluded_patterns", DEFAULT_EXCLUDED_PATTERNS)
    if not isinstance(patterns, list) or not all(isinstance(item, str) and item for item in patterns):
        raise ValueError("detailed-capture editor.excluded_patterns must be a list of strings")
    config["editor"]["excluded_patterns"] = patterns
    return config


def load(path: Path | None = None) -> dict[str, Any]:
    path = path or config_path()
    if not path.exists():
        return deepcopy(DEFAULT_CONFIG)
    try:
        return _normalise(json.loads(path.read_text(encoding="utf-8")))
    except json.JSONDecodeError as exc:
        raise ValueError(f"invalid detailed-capture config: {path}") from exc


def save(config: dict[str, Any], path: Path | None = None) -> Path:
    path = path or config_path()
    normalised = _normalise(config)
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile("w", encoding="utf-8", dir=path.parent, delete=False) as temporary:
        json.dump(normalised, temporary, indent=2)
        temporary.write("\n")
        temporary_path = Path(temporary.name)
    temporary_path.replace(path)
    return path


def set_enabled(kind: str, enabled: bool, path: Path | None = None) -> dict[str, Any]:
    if kind not in {"editor", "browser", "filesystem"}:
        raise ValueError("detailed-capture kind must be editor or browser")
    config = load(path)
    config[kind]["enabled"] = enabled
    save(config, path)
    return config


def set_browser_context_enabled(enabled: bool, path: Path | None = None) -> dict[str, Any]:
    config = load(path)
    config["browser"]["context_enabled"] = enabled
    save(config, path)
    return config


def is_detailed_event(source: str, event_type: str) -> bool:
    return (source, event_type) in DETAILED_EVENT_KINDS


def is_enabled(source: str, event_type: str, config: dict[str, Any]) -> bool:
    if (source, event_type) == ("vscode", "document_change"):
        return bool(config["editor"]["enabled"])
    if (source, event_type) == ("firefox", "user_action"):
        return bool(config["browser"]["enabled"])
    if (source, event_type) == ("filesystem", "file_content"):
        return bool(config["filesystem"]["enabled"])
    return True


def approved_workspaces() -> list[str]:
    """Read existing workspace consent without making the server own that setting."""
    try:
        return load_workspaces()["workspaces"]
    except (ImportError, OSError, ValueError):
        return []




def editor_event_is_approved(payload: dict[str, Any], config: dict[str, Any]) -> bool:
    """Require an approved workspace and default/custom excluded-path rules server-side."""
    try:
        candidate = Path(payload["path"]).expanduser().resolve()
        workspace = Path(payload["workspace"]).expanduser().resolve()
        candidate.relative_to(workspace)
    except (KeyError, TypeError, ValueError):
        return False
    approved = {Path(item).expanduser().resolve() for item in approved_workspaces()}
    if workspace not in approved:
        return False
    normalised = str(candidate).lower()
    basename = candidate.name.lower()
    if (
        basename == ".env"
        or basename.startswith(".env.")
        or basename.endswith(".pem")
        or basename.endswith(".key")
        or basename.startswith("id_rsa")
        or any(word in normalised for word in ("secret", "credential", "password", "token"))
    ):
        return False
    return not any(fnmatch(basename, pattern.lower()) for pattern in config["editor"]["excluded_patterns"])

def public_config(config: dict[str, Any]) -> dict[str, Any]:
    """Return non-secret configuration consumed by local extensions and status."""
    from .retention_policy import load as load_retention_policy, public_summary

    try:
        retention = public_summary(load_retention_policy())
    except ValueError:
        retention = "indefinite"
    return {
        "editor": {
            "enabled": config["editor"]["enabled"],
            "excluded_patterns": config["editor"]["excluded_patterns"],
        },
        "browser": {"enabled": config["browser"]["enabled"], "context_enabled": config["browser"]["context_enabled"]},
        "filesystem": {"enabled": config["filesystem"]["enabled"]},
        "approved_workspaces": approved_workspaces(),
        "retention": retention,
        "export_includes_details": True,
    }
