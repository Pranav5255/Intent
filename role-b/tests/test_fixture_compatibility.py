from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parents[1]))

from intent_engine.normalize import normalize_event
from intent_engine.schemas import DayExport, RawEvent
from intent_engine.source import load_replay_fixture


FIXTURE_PATH = Path(__file__).parent / "fixtures" / "demo-day.json"


def test_load_fixture_demo_day_json() -> None:
    export = load_replay_fixture(str(FIXTURE_PATH))

    assert export.date == "2026-07-13"
    assert len(export.events) > 0
    print(f"Loaded {len(export.events)} events")


def test_firefox_source_mapping() -> None:
    event = RawEvent(
        id="firefox-event",
        ts=1,
        source="firefox",
        type="tab_change",
        payload={"url": "https://example.com/docs", "title": "Documentation"},
    )

    normalized, warning = normalize_event(event, ordinal=0)

    assert warning is None
    assert normalized is not None
    assert normalized.family == "browser"


def test_chrome_source_mapping() -> None:
    event = RawEvent(
        id="chrome-event",
        ts=1,
        source="chrome",
        type="tab_change",
        payload={"url": "https://example.com/docs", "title": "Documentation"},
    )

    assert event.source == "chrome"
    assert event.type == "tab_change"


def test_unknown_event_retained() -> None:
    event = RawEvent(id="unknown-event", ts=1, source="unknown", type="unknown", payload={})

    assert event.source == "unknown"
    assert event.type == "unknown"


def test_redacted_url_preserved() -> None:
    event = RawEvent(
        id="redacted-event",
        ts=1,
        source="firefox",
        type="tab_change",
        payload={"url": "[REDACTED]"},
    )

    assert event.payload.model_extra == {"url": "[REDACTED]"}


def test_missing_detailed_events_ok() -> None:
    export = DayExport(
        date="2026-07-13",
        exported_at=1,
        events=[RawEvent(
            id="focus-event",
            ts=1,
            source="linux",
            type="app_focus",
            payload={"app": "code", "title": "main.py"},
        )],
    )

    assert len(export.events) == 1
    print("Missing detailed events OK")
