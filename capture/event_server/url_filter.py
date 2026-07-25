"""Domain-based privacy filter for browser events.

The filter deliberately compares parsed host names rather than URL substrings:
``purchase.com`` must never match a ``chase`` rule.
"""

from __future__ import annotations

import os
from copy import deepcopy
from pathlib import Path
from typing import Any
from urllib.parse import urlsplit


DEFAULT_CONFIG = {"blocked_domains": []}


def config_path(home: Path | None = None) -> Path:
    override = os.environ.get("INTENT_BLOCKED_DOMAINS_CONFIG")
    if override:
        return Path(override).expanduser()
    home = home or Path.home()
    config_home = Path(os.environ.get("XDG_CONFIG_HOME", home / ".config"))
    return config_home / "intent" / "blocked-domains.yaml"


def _strip_comment(line: str) -> str:
    """Remove comments in the small, intentionally simple shipped YAML format."""
    return line.split("#", maxsplit=1)[0].strip()


def load(path: Path | None = None) -> dict[str, list[str]]:
    """Load a ``blocked_domains`` YAML list without adding a runtime dependency."""
    path = path or config_path()
    if not path.exists():
        return deepcopy(DEFAULT_CONFIG)
    lines = [_strip_comment(line) for line in path.read_text(encoding="utf-8").splitlines()]
    values: list[str] = []
    in_blocked_domains = False
    for line in lines:
        if not line:
            continue
        if line == "blocked_domains:" or line.startswith("blocked_domains: #"):
            in_blocked_domains = True
            continue
        if line.startswith("-") and in_blocked_domains:
            value = line[1:].strip().strip('"\'')
            if not value:
                raise ValueError("blocked-domains entries must be non-empty strings")
            values.append(value)
            continue
        raise ValueError(f"invalid blocked-domains config: {path}")
    if not in_blocked_domains:
        raise ValueError("blocked-domains config must contain blocked_domains")
    return {"blocked_domains": normalise_patterns(values)}


def normalise_patterns(patterns: list[str]) -> list[str]:
    normalised: list[str] = []
    for pattern in patterns:
        if not isinstance(pattern, str):
            raise ValueError("blocked-domains entries must be strings")
        value = pattern.strip().lower().rstrip(".")
        if value.startswith("*."):
            value = value[2:]
        if not value or any(character in value for character in "/:@?#["):
            raise ValueError("blocked-domains entries must be host patterns")
        if value not in normalised:
            normalised.append(value)
    return normalised


def _host(url: str) -> str | None:
    try:
        parsed = urlsplit(url)
        return parsed.hostname.lower().rstrip(".") if parsed.hostname else None
    except ValueError:
        return None


def is_url_blocked(url: str, patterns: list[str]) -> bool:
    """Return whether a URL host exactly matches or is a subdomain of a rule.

    A no-TLD rule (for example ``chase``) also matches its conventional ``.com``
    host and subdomains, mirroring the screenpipe matching behaviour.
    """
    host = _host(url)
    if not host:
        return False
    for pattern in normalise_patterns(patterns):
        candidates = (pattern, f"{pattern}.com") if "." not in pattern else (pattern,)
        if any(host == candidate or host.endswith(f".{candidate}") for candidate in candidates):
            return True
    return False


def redact_blocked_browser_event(payload: dict[str, Any]) -> dict[str, Any]:
    """Keep audit-safe browser metadata after a configured domain match."""
    safe = deepcopy(payload)
    safe["url"] = "[blocked]"
    safe["blocked"] = True
    # Semantic browser actions may contain form labels or page excerpts.  Preserve
    # only the event mechanics required by the producer contract.
    safe.pop("context", None)
    target = safe.get("target")
    if isinstance(target, dict):
        safe["target"] = {field: target[field] for field in ("tag", "role") if field in target}
    return safe
