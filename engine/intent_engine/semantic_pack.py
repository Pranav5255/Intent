"""Deterministic, privacy-bounded packets for future semantic clustering."""

from __future__ import annotations

from typing import Any, Literal

from pydantic import BaseModel, Field

from intent_engine.providers import semantic_content_consent_granted, semantic_full_capture_consent_granted
from intent_engine.schemas import NormalizedEvent


MAX_EVENTS_PER_PACKET = 12
MAX_SNIPPET_CHARS_PER_EVENT = 500
MAX_SNIPPET_CHARS_PER_PACKET = 2000

_MESSAGING_MARKERS = ("whatsapp", "telegram", "signal", "slack", "discord", "teams")
_MEDIA_MARKERS = ("spotify", "vlc", "rhythmbox", "media player", "music.apple.com", "music.youtube.com")
_SAFE_BROWSER_ACTIONS = {
    "click", "link_activation", "form_submit", "toggle", "select_change", "scroll",
    "like", "reply", "repost", "share", "follow", "unfollow",
}


class SemanticPacketEvent(BaseModel):
    """A limited event representation safe for a future semantic provider."""

    event_id: str
    ts: int
    family: str
    category: str
    project_paths: list[str] = Field(default_factory=list)
    file_name: str | None = None
    domain: str | None = None
    command_family: str | None = None
    safe_metadata: dict[str, str | int | bool | None] = Field(default_factory=dict)
    deterministic_role: Literal["candidate", "background"]
    content_snippet: str | None = None
    # Present only under explicit full-capture cloud consent. This is the
    # complete Role A event, including its envelope and payload. The compact
    # provider codec lives in semantic_cluster.py and never persists this
    # field separately.
    captured_event: dict[str, Any] | None = None


class SemanticCandidatePacket(BaseModel):
    """A chronologically adjacent packet of future semantic candidates."""

    start_ts: int
    end_ts: int
    events: list[SemanticPacketEvent] = Field(default_factory=list)


def build_semantic_candidate_packets(
    session: list[NormalizedEvent], max_gap_minutes: int = 5
) -> list[SemanticCandidatePacket]:
    """Build bounded, in-memory candidate packets without contacting an LLM."""

    if max_gap_minutes < 0:
        raise ValueError("max_gap_minutes must be non-negative")

    packets: list[SemanticCandidatePacket] = []
    candidates: list[NormalizedEvent] = []
    gap_seconds = max_gap_minutes * 60
    full_capture = semantic_full_capture_consent_granted()

    def flush_candidates() -> None:
        nonlocal candidates
        if candidates:
            packets.append(_build_packet(candidates, "candidate", full_capture=full_capture))
            candidates = []

    for event in sorted(session, key=lambda item: (item.ts, item.ordinal)):
        classification = _classify_event(event)
        if classification == "excluded" and not full_capture:
            continue
        if classification == "excluded":
            classification = "candidate"
        if classification == "background":
            flush_candidates()
            packets.append(_build_packet([event], "background", full_capture=full_capture))
            continue

        if candidates and (event.ts - candidates[-1].ts > gap_seconds or len(candidates) >= MAX_EVENTS_PER_PACKET):
            flush_candidates()
        candidates.append(event)

    flush_candidates()
    return packets


def _build_packet(
    events: list[NormalizedEvent], role: Literal["candidate", "background"], *, full_capture: bool = False
) -> SemanticCandidatePacket:
    remaining_snippet_chars = MAX_SNIPPET_CHARS_PER_PACKET
    packet_events: list[SemanticPacketEvent] = []
    for event in events:
        snippet = None
        if not full_capture and role == "candidate" and remaining_snippet_chars:
            snippet = _content_snippet(event, remaining_snippet_chars)
            if snippet:
                remaining_snippet_chars -= len(snippet)
        packet_events.append(_packet_event(event, role, snippet, full_capture=full_capture))
    return SemanticCandidatePacket(start_ts=events[0].ts, end_ts=events[-1].ts, events=packet_events)


def _packet_event(
    event: NormalizedEvent,
    role: Literal["candidate", "background"],
    snippet: str | None,
    *,
    full_capture: bool = False,
) -> SemanticPacketEvent:
    return SemanticPacketEvent(
        event_id=event.id,
        ts=event.ts,
        family=event.family,
        category=event.category,
        project_paths=list(event.entities.project_paths),
        file_name=event.entities.file_name,
        domain=event.entities.domain,
        command_family=event.entities.command_family,
        safe_metadata={
            "source": event.source,
            "file_kind": event.entities.file_kind,
            "typed_chars": event.signals.typed_chars,
            "save": event.signals.save,
            "todo_added": event.signals.todo_added,
            "exit_code": event.entities.exit_code,
            "action": _browser_action(event),
        },
        deterministic_role=role,
        content_snippet=snippet,
        captured_event=_captured_event(event) if full_capture else None,
    )


def _captured_event(event: NormalizedEvent) -> dict[str, Any]:
    """Return the complete captured Role A event under explicit cloud consent."""

    raw = event.raw if isinstance(event.raw, dict) else {}
    return raw


def _browser_action(event: NormalizedEvent) -> str | None:
    """Allowlist interaction mechanics without exposing page or target content."""

    if event.family != "browser" or event.category != "user_action":
        return None
    payload = event.raw.get("payload") if isinstance(event.raw, dict) else None
    action = payload.get("action") if isinstance(payload, dict) else None
    return action if isinstance(action, str) and action in _SAFE_BROWSER_ACTIONS else None


def _classify_event(event: NormalizedEvent) -> Literal["candidate", "background", "excluded"]:
    identifiers = _local_identifiers(event)
    if _matches_marker(identifiers, _MESSAGING_MARKERS):
        return "excluded"
    if _matches_marker(identifiers, _MEDIA_MARKERS):
        return "background"
    return "candidate"


def _local_identifiers(event: NormalizedEvent) -> list[str]:
    payload = event.raw.get("payload") if isinstance(event.raw, dict) else None
    raw_payload = payload if isinstance(payload, dict) else {}
    values = [event.source, event.entities.domain or "", event.entities.title or ""]
    for key in ("app", "title"):
        value = raw_payload.get(key)
        if isinstance(value, str):
            values.append(value)
    return [value.lower() for value in values]


def _matches_marker(identifiers: list[str], markers: tuple[str, ...]) -> bool:
    return any(marker in value for value in identifiers for marker in markers)


def _content_snippet(event: NormalizedEvent, remaining_chars: int) -> str | None:
    if not semantic_content_consent_granted():
        return None
    payload = event.raw.get("payload") if isinstance(event.raw, dict) else None
    if not isinstance(payload, dict):
        return None
    if event.family == "editor" and event.category == "document_change":
        return _editor_snippet(payload, remaining_chars)
    if event.family == "browser" and event.category == "user_action":
        return _browser_snippet(payload, remaining_chars)
    return None


def _editor_snippet(payload: dict, remaining_chars: int) -> str | None:
    changes = payload.get("changes")
    if not isinstance(changes, list):
        return None
    fragments = [
        change["text"]
        for change in changes
        if isinstance(change, dict)
        and not change.get("redacted")
        and isinstance(change.get("text"), str)
        and change["text"] != "[redacted]"
    ]
    return _bounded_snippet("\n".join(fragments), remaining_chars)


def _browser_snippet(payload: dict, remaining_chars: int) -> str | None:
    if payload.get("sensitive_page") or payload.get("blocked"):
        return None
    context = payload.get("context")
    if not isinstance(context, dict):
        return None
    excerpt = context.get("text_excerpt")
    return _bounded_snippet(excerpt, remaining_chars) if isinstance(excerpt, str) else None


def _bounded_snippet(value: str, remaining_chars: int) -> str | None:
    normalized = " ".join(value.split())
    if not normalized or normalized == "[redacted]":
        return None
    limit = min(MAX_SNIPPET_CHARS_PER_EVENT, remaining_chars)
    return normalized[:limit] or None
