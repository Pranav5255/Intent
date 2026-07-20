from __future__ import annotations

import asyncio
import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parents[1]))

import intent_engine.current as current_module
from intent_engine.current import CurrentIntentEngine
from intent_engine.schemas import EventPayload, RawEvent
from intent_engine.source import RoleAUnavailableError


class FakeClient:
    def __init__(self, events=None, unavailable=False):
        self.events = events or []
        self.unavailable = unavailable
        self.since_values = []

    async def fetch_events_since(self, since_ts: int):
        self.since_values.append(since_ts)
        if self.unavailable:
            raise RoleAUnavailableError("offline")
        return self.events


def make_events(count: int, start: int = 9_000) -> list[RawEvent]:
    return [
        RawEvent(
            id=f"event-{index}",
            ts=start + index,
            source="vscode",
            type="edit",
            payload=EventPayload(file_path="/repo/iam.tf"),
        )
        for index in range(count)
    ]


def run(coroutine):
    return asyncio.run(coroutine)


@pytest.mark.parametrize(("count", "confidence"), [(3, 0.6), (5, 0.6), (6, 0.8)])
def test_current_confidence_buckets_and_since_timestamp(monkeypatch, count, confidence) -> None:
    monkeypatch.setattr(current_module.time, "time", lambda: 10_000.0)
    client = FakeClient(make_events(count))
    result = run(CurrentIntentEngine(client).get_current())

    assert result is not None
    assert result.confidence == confidence
    assert result.since_ts == 9_000
    assert client.since_values == [8_200]


def test_current_low_confidence_and_empty_results_return_none(monkeypatch) -> None:
    monkeypatch.setattr(current_module.time, "time", lambda: 10_000.0)
    assert run(CurrentIntentEngine(FakeClient(make_events(2))).get_current()) is None
    assert run(CurrentIntentEngine(FakeClient()).get_current()) is None


def test_current_cache_reuses_for_60_seconds_then_refreshes(monkeypatch) -> None:
    now = [10_000.0]
    monkeypatch.setattr(current_module.time, "time", lambda: now[0])
    client = FakeClient(make_events(3))
    engine = CurrentIntentEngine(client)

    first = run(engine.get_current())
    second = run(engine.get_current())
    assert first == second
    assert len(client.since_values) == 1

    now[0] = 10_060.0
    third = run(engine.get_current())
    assert third is not None
    assert len(client.since_values) == 2


def test_current_role_a_unavailable_returns_none() -> None:
    assert run(CurrentIntentEngine(FakeClient(unavailable=True)).get_current()) is None


def test_current_provider_receives_safe_feature_packet(monkeypatch) -> None:
    class RecordingProvider:
        def __init__(self) -> None:
            self.calls = []

        async def label_cluster(self, cluster_events_text, project_tag=None, hints=None):
            self.calls.append({"text": cluster_events_text, "project_tag": project_tag, "hints": hints})
            return {"label": "Safe Current Work", "summary": "Inferred current activity.", "confidence": 0.8}

    monkeypatch.setattr(current_module.time, "time", lambda: 10_000.0)
    provider = RecordingProvider()
    monkeypatch.setattr(current_module, "create_label_provider", lambda: provider)
    client = FakeClient([
        RawEvent(
            id=f"event-{index}", ts=9_000 + index, source="vscode", type="document_change",
            payload=EventPayload(path="/repo/private/iam.tf", changes=[{"text": "SECRET_EDITOR_SOURCE"}]),
        )
        for index in range(3)
    ])

    result = run(CurrentIntentEngine(client).get_current())

    assert result is not None
    packet = json.loads(provider.calls[0]["text"])
    assert packet["policy_version"] == "safe-intent-features-v1"
    captured = json.dumps(provider.calls)
    assert "/repo/private/iam.tf" not in captured
    assert "SECRET_EDITOR_SOURCE" not in captured
    assert provider.calls[0]["project_tag"] is None
    assert "top_file" not in (provider.calls[0]["hints"] or {})
