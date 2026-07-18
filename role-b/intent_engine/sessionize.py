"""Deterministic chronological grouping for normalized activity events."""

from __future__ import annotations

from intent_engine.schemas import NormalizedEvent


async def sessionize(events: list[NormalizedEvent], gap_minutes: int = 15) -> list[list[NormalizedEvent]]:
    """Group ordered events into activity sessions separated by gaps or idle markers."""

    if not events:
        return []
    if len(events) == 1:
        return [[events[0]]]

    gap_seconds = gap_minutes * 60
    sessions: list[list[NormalizedEvent]] = []
    current_session: list[NormalizedEvent] = []

    for event in events:
        if event.family == "idle" and event.category in {"idle_start", "idle_end"}:
            if current_session:
                sessions.append(current_session)
                current_session = []
            continue

        if not current_session:
            current_session = [event]
        elif event.ts - current_session[-1].ts > gap_seconds:
            sessions.append(current_session)
            current_session = [event]
        else:
            current_session.append(event)

    if current_session:
        sessions.append(current_session)
    return sessions
