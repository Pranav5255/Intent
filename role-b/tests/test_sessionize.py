from __future__ import annotations

import asyncio
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parents[1]))

from intent_engine.schemas import NormalizedEvent
from intent_engine.sessionize import sessionize


def event(event_id: str, ts: int, family: str = "editor", category: str = "file_edit") -> NormalizedEvent:
    return NormalizedEvent(
        id=event_id,
        ts=ts,
        ordinal=0,
        source="test",
        family=family,
        category=category,
        text="Test event",
        raw={},
    )


def run(events: list[NormalizedEvent], gap_minutes: int = 15) -> list[list[NormalizedEvent]]:
    return asyncio.run(sessionize(events, gap_minutes=gap_minutes))


def test_empty_and_single_event_inputs() -> None:
    single = event("one", 1)

    assert run([]) == []
    assert run([single]) == [[single]]


def test_gap_boundaries_and_nearby_focus_switches() -> None:
    events = [
        event("editor", 0),
        event("focus", 300, family="focus", category="app_focus"),
        event("at-threshold", 900, family="browser", category="tab_change"),
        event("after-threshold", 1801, family="command", category="command"),
    ]

    sessions = run(events)

    assert [[item.id for item in session] for session in sessions] == [
        ["editor", "focus", "at-threshold"],
        ["after-threshold"],
    ]


def test_idle_markers_are_boundaries_not_sessions() -> None:
    events = [
        event("before-idle", 0),
        event("idle-start", 10, family="idle", category="idle_start"),
        event("idle-end", 100, family="idle", category="idle_end"),
        event("after-idle", 110),
        event("idle-start-2", 120, family="idle", category="idle_start"),
        event("idle-end-2", 130, family="idle", category="idle_end"),
        event("after-idle-2", 140),
    ]

    sessions = run(events)

    assert [[item.id for item in session] for session in sessions] == [
        ["before-idle"],
        ["after-idle"],
        ["after-idle-2"],
    ]
    assert all(item.family != "idle" for session in sessions for item in session)


def test_zero_gap_splits_increasing_timestamps() -> None:
    sessions = run([event("first", 1), event("second", 2)], gap_minutes=0)

    assert [[item.id for item in session] for session in sessions] == [["first"], ["second"]]
