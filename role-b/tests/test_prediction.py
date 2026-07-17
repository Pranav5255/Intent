from __future__ import annotations

import asyncio
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parents[1]))

from intent_engine.prediction import PredictionEngine
from intent_engine.schemas import EventEntities, EventSignals, Intent, IntentInsights, IntentStats, NormalizedEvent, ResumePayload


def event(index: int, family: str = "editor", category: str = "edit", command: str | None = None) -> NormalizedEvent:
    return NormalizedEvent(
        id=f"event-{index}", ts=index, ordinal=index, source="vscode", family=family,
        category=category, text="Edited file", entities=EventEntities(command_family=command), signals=EventSignals(), raw={},
    )


def intent(intent_id: str, end_ts: int, prefix: tuple[str, str, str]) -> Intent:
    return Intent(
        id=intent_id, date="2026-07-16", label=f"Intent {intent_id}", summary="Historical work.",
        start_ts=end_ts - 10, end_ts=end_ts, depth=1, prefix=prefix,
        stats=IntentStats(event_count=3, duration_seconds=10), insights=IntentInsights(), resume_payload=ResumePayload(files=[f"{intent_id}.py"]),
    )


class Store:
    def __init__(self, roots):
        self.roots = roots

    async def get_intents_by_date(self, date):
        return self.roots


def run(coroutine):
    return asyncio.run(coroutine)


def test_training_indexes_children_and_predicts_latest_match() -> None:
    prefix = ("editor", "edit", "")
    child_one = intent("old", 10, prefix)
    child_two = intent("new", 20, prefix)
    root = intent("root", 30, ("other", "session", ""))
    root.children = [child_one, child_two]
    engine = PredictionEngine(Store([root]))
    run(engine.train_on_date("2026-07-16"))

    result = run(engine.predict([event(1), event(2), event(3)]))
    assert result is not None
    assert result.predicted_label == "Intent new"
    assert result.confidence == 0.7
    assert result.resume_payload.files == ["new.py"]


def test_prediction_requires_three_events_and_two_matches() -> None:
    prefix = ("editor", "edit", "")
    engine = PredictionEngine(Store([intent("only", 10, prefix)]))
    run(engine.train_on_date("2026-07-16"))
    assert run(engine.predict([event(1), event(2)])) is None
    assert run(engine.predict([event(1), event(2), event(3)])) is None


def test_prediction_confidence_increases_with_multiple_matches() -> None:
    prefix = ("editor", "edit", "")
    matches = [intent(str(index), index, prefix) for index in range(2, 7)]
    engine = PredictionEngine(Store(matches))
    run(engine.train_on_date("2026-07-16"))
    result = run(engine.predict([event(1), event(2), event(3)]))
    assert result is not None and result.confidence == 0.95
