import asyncio
import os
import tempfile
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

os.environ.setdefault("ROLE_B_DB_PATH", str(Path(tempfile.gettempdir()) / "role-b-forgetting-import.db"))

from intent_engine.api import create_app
from intent_engine.schemas import Intent, IntentInsights, IntentStats, PipelineResult, ResumePayload
from intent_engine.store import IntentStore


def make_intent(intent_id: str, date: str, label: str, tags: list[str]) -> Intent:
    return Intent(
        id=intent_id, date=date, label=label, summary=f"Summary {label}", start_ts=1, end_ts=10,
        depth=0, tags=tags, stats=IntentStats(event_count=2, duration_seconds=9),
        insights=IntentInsights(), resume_payload=ResumePayload(),
    )


def run(awaitable):
    return asyncio.run(awaitable)


def save(store, root):
    run(store.save_pipeline_run(root.date, PipelineResult(intents=[root], source_hash=root.id, pipeline_version="v1")))


def test_delete_date_purges_intents_search_and_fts():
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        removed = make_intent("removed", "2026-07-13", "Unique Forgettable", ["project:infra"])
        remaining = make_intent("remaining", "2026-07-14", "Keep This", ["project:docs"])
        save(store, removed)
        save(store, remaining)

        result = run(store.delete_date("2026-07-13"))
        assert result == {"deleted_intent_ids": ["removed"], "deleted_count": 1}
        assert run(store.get_intents_by_date("2026-07-13")) == []
        assert run(store.get_intent_by_id("removed")) is None
        assert run(store.search_intents("Unique Forgettable")) == []
        assert [item.id for item in run(store.get_intents_by_date("2026-07-14"))] == ["remaining"]
        assert run(store.delete_date("2026-07-13"))["deleted_count"] == 0


def test_delete_project_purges_matching_tags_and_pipeline_cache():
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        save(store, make_intent("infra", "2026-07-13", "Infra Secret", ["project:infra"] ))
        save(store, make_intent("docs", "2026-07-14", "Docs Work", ["project:docs"] ))
        result = run(store.delete_project("infra"))
        assert result == {"deleted_intent_ids": ["infra"], "deleted_count": 1}
        assert run(store.get_intent_by_id("infra")) is None
        assert run(store.search_intents("Infra Secret")) == []
        assert run(store.get_intent_by_id("docs")) is not None


def test_forgetting_api_routes_and_invalid_date():
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        save(store, make_intent("api-date", "2026-07-13", "API Forget", ["project:api"]))
        client = TestClient(create_app(store=store))
        response = client.delete("/v1/memory/date/2026-07-13")
        assert response.status_code == 200
        assert response.json()["deleted_count"] == 1
        assert client.delete("/v1/memory/date/2026-02-30").status_code == 400
