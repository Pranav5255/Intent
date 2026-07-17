import asyncio
import os
import tempfile
from pathlib import Path

from fastapi.testclient import TestClient
import pytest

os.environ.setdefault("ROLE_B_DB_PATH", str(Path(tempfile.gettempdir()) / "role-b-intent-stats-import.db"))

from intent_engine.api import create_app
from intent_engine.schemas import Intent, IntentInsights, IntentStats, PipelineResult, ResumePayload
from intent_engine.store import IntentStore


def make_intent(intent_id: str, date: str, label: str, tag: str, events: int, duration: int) -> Intent:
    return Intent(
        id=intent_id,
        date=date,
        label=label,
        summary=f"Summary for {label}",
        start_ts=1,
        end_ts=duration,
        depth=0,
        tags=[tag],
        stats=IntentStats(event_count=events, duration_seconds=duration),
        insights=IntentInsights(),
        resume_payload=ResumePayload(),
    )


def run(awaitable):
    return asyncio.run(awaitable)


def test_intent_stats_aggregate_and_project_filter():
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        run(store.save_pipeline_run("2026-07-13", PipelineResult(
            intents=[make_intent("one", "2026-07-13", "Work Task", "project:infra", 4, 10)],
            source_hash="one", pipeline_version="v1",
        )))
        run(store.save_pipeline_run("2026-07-14", PipelineResult(
            intents=[make_intent("two", "2026-07-14", "Work Task", "project:docs", 3, 20)],
            source_hash="two", pipeline_version="v1",
        )))

        stats = run(store.get_intent_stats("2026-07-13", "2026-07-14"))
        assert stats["intent_count"] == 2
        assert stats["total_duration_seconds"] == 30
        assert stats["event_count"] == 7
        assert stats["by_date"] == [
            {"date": "2026-07-13", "intent_count": 1, "duration_seconds": 10},
            {"date": "2026-07-14", "intent_count": 1, "duration_seconds": 20},
        ]
        assert stats["top_labels"] == [{"label": "Work Task", "count": 2}]
        filtered = run(store.get_intent_stats("2026-07-13", "2026-07-14", "infra"))
        assert filtered["intent_count"] == 1
        assert filtered["project"] == "infra"

        with pytest.raises(ValueError):
            run(store.get_intent_stats("2026-07-14", "2026-07-13"))


def test_intent_stats_api_and_invalid_range():
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        client = TestClient(create_app(store=store))
        assert client.get("/intents/stats", params={"date_from": "2026-02-30", "date_to": "2026-07-14"}).status_code == 400
