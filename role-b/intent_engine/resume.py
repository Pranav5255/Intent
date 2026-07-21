"""Deterministic restore-context payloads derived from normalized events."""

from __future__ import annotations

from urllib.parse import urlsplit

from intent_engine.schemas import NormalizedEvent, ResumePayload

RESUME_POLICY_VERSION = "2"
_MAX_RESTORE_URLS = 8


def build_resume_payload(cluster: list[NormalizedEvent]) -> ResumePayload:
    """Build a bounded restore payload with the last observed URL of each tab."""

    files: list[str] = []
    urls: list[str] = []
    seen_files: set[str] = set()
    seen_urls: set[str] = set()
    seen_tab_ids: set[int] = set()
    closed_tab_ids: set[int] = set()
    closed_urls: set[str] = set()
    shell: dict[str, str] = {}

    for event in sorted(cluster, key=lambda item: (item.ts, item.ordinal), reverse=True):
        if event.family == "editor" and event.entities.file_path:
            path = event.entities.file_path
            if path not in seen_files and len(files) < 5:
                seen_files.add(path)
                files.append(path)

        if event.family == "browser":
            url = _event_url(event)
            tab_id = _event_tab_id(event)
            if event.category == "tab_close":
                if tab_id is not None:
                    closed_tab_ids.add(tab_id)
                if url:
                    closed_urls.add(url)
                continue
            if tab_id is not None:
                if tab_id in seen_tab_ids or tab_id in closed_tab_ids:
                    continue
                # Mark the tab even when its final URL is shared with another
                # tab, so an older URL from this tab is not restored instead.
                seen_tab_ids.add(tab_id)
            if (
                url
                and _restorable_domain(url)
                and url not in seen_urls
                and url not in closed_urls
                and len(urls) < _MAX_RESTORE_URLS
            ):
                seen_urls.add(url)
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
    seen_urls: set[str] = set()

    for payload in reversed(payloads):
        for path in payload.files:
            if path not in seen_files and len(files) < 5:
                seen_files.add(path)
                files.append(path)

        for url in payload.urls:
            if url not in seen_urls and _restorable_domain(url) and len(urls) < _MAX_RESTORE_URLS:
                seen_urls.add(url)
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


def _event_tab_id(event: NormalizedEvent) -> int | None:
    payload = event.raw.get("payload")
    tab_id = payload.get("tab_id") if isinstance(payload, dict) else None
    return tab_id if isinstance(tab_id, int) and not isinstance(tab_id, bool) and tab_id >= 0 else None


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
