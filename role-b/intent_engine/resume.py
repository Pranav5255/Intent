"""Deterministic restore-context payloads derived from normalized events."""

from __future__ import annotations

from urllib.parse import urlsplit

from intent_engine.schemas import NormalizedEvent, ResumePayload


def build_resume_payload(cluster: list[NormalizedEvent]) -> ResumePayload:
    """Build a bounded, most-recent-first restore payload for one child intent."""

    files: list[str] = []
    urls: list[str] = []
    seen_files: set[str] = set()
    seen_domains: set[str] = set()
    shell: dict[str, str] = {}

    for event in sorted(cluster, key=lambda item: (item.ts, item.ordinal), reverse=True):
        if event.family == "editor" and event.entities.file_path:
            path = event.entities.file_path
            if path not in seen_files and len(files) < 5:
                seen_files.add(path)
                files.append(path)

        if event.family == "browser":
            url = _event_url(event)
            domain = _restorable_domain(url)
            if url and domain and domain not in seen_domains and len(urls) < 8:
                seen_domains.add(domain)
                urls.append(url)

        if event.family == "command" and not shell:
            if event.entities.cwd:
                shell["cwd"] = event.entities.cwd
            if event.entities.command:
                shell["last_cmd"] = event.entities.command

    return ResumePayload(files=files, urls=urls, shell=shell)


def merge_resume_payloads(payloads: list[ResumePayload]) -> ResumePayload:
    """Merge chronological child payloads, treating the final payload as newest."""

    files: list[str] = []
    urls: list[str] = []
    shell: dict[str, str] = {}
    seen_files: set[str] = set()
    seen_domains: set[str] = set()

    for payload in reversed(payloads):
        for path in payload.files:
            if path not in seen_files and len(files) < 5:
                seen_files.add(path)
                files.append(path)

        for url in payload.urls:
            domain = _restorable_domain(url)
            if domain and domain not in seen_domains and len(urls) < 8:
                seen_domains.add(domain)
                urls.append(url)

        for key in ("cwd", "last_cmd"):
            value = payload.shell.get(key)
            if key not in shell and isinstance(value, str) and value:
                shell[key] = value

    return ResumePayload(files=files, urls=urls, shell=shell)


def _event_url(event: NormalizedEvent) -> str | None:
    payload = event.raw.get("payload")
    url = payload.get("url") if isinstance(payload, dict) else None
    return url if isinstance(url, str) else None


def _restorable_domain(url: str | None) -> str | None:
    if not url or "[redacted]" in url.lower():
        return None
    try:
        parsed = urlsplit(url)
    except ValueError:
        return None
    if parsed.scheme not in {"http", "https"} or not parsed.hostname:
        return None
    return parsed.hostname.lower()
