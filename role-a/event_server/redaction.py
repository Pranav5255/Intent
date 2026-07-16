"""Server-side final redaction for detailed editor and browser events."""

from __future__ import annotations

import re
from copy import deepcopy
from urllib.parse import urlsplit, urlunsplit

from .models import EventIn


PRIVATE_KEY = re.compile(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----", re.IGNORECASE)
SENSITIVE_ASSIGNMENT = re.compile(
    r"\b(?:password|passwd|secret|token|api[_-]?key|access[_-]?key|authorization)\b\s*[:=]",
    re.IGNORECASE,
)
TOKEN_PREFIX = re.compile(r"\b(?:gh[pous]_[A-Za-z0-9_]+|xox[baprs]-[A-Za-z0-9-]+|eyJ[A-Za-z0-9_-]+\.)")


def sanitize_url(raw_url: str) -> str | None:
    """Retain only an HTTP(S) origin/path without credentials, query or fragment."""
    try:
        parsed = urlsplit(raw_url)
    except ValueError:
        return None
    if parsed.scheme not in {"http", "https"} or not parsed.hostname:
        return None
    hostname = parsed.hostname
    if parsed.port:
        hostname = f"{hostname}:{parsed.port}"
    return urlunsplit((parsed.scheme, hostname, parsed.path or "/", "", ""))


def should_redact_text(text: str) -> bool:
    return bool(PRIVATE_KEY.search(text) or SENSITIVE_ASSIGNMENT.search(text) or TOKEN_PREFIX.search(text))


def redact_event(event: EventIn) -> EventIn:
    """Return a copy safe to store; never mutate the connector's input object."""
    payload = deepcopy(event.payload)
    if event.source == "vscode" and event.type == "document_change":
        for change in payload.get("changes", []):
            text = change.get("text")
            if isinstance(text, str) and should_redact_text(text):
                change["text"] = "[redacted]"
                change["redacted"] = True

    if event.source == "firefox":
        url = payload.get("url")
        if isinstance(url, str):
            clean_url = sanitize_url(url)
            if clean_url is None:
                raise ValueError("firefox event url must be an http or https URL")
            payload["url"] = clean_url
        target = payload.get("target")
        if isinstance(target, dict) and isinstance(target.get("href"), str):
            clean_href = sanitize_url(target["href"])
            if clean_href is None:
                target.pop("href", None)
            else:
                target["href"] = clean_href
        context = payload.get("context")
        if isinstance(context, dict) and isinstance(context.get("text_excerpt"), str) and should_redact_text(context["text_excerpt"]):
            payload.pop("context", None)

    copier = getattr(event, "model_copy", event.copy)
    return copier(update={"payload": payload})
