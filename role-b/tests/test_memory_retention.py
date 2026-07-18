from __future__ import annotations

import asyncio
import json
import tempfile
from pathlib import Path

import pytest

from intent_engine.schemas import Intent, IntentInsights, IntentStats, PipelineResult, ResumePayload
from intent_engine.store import IntentStore


def run(awaitable):
    return asyncio.run(awaitable)


def make_intent(intent_id: str, date: str, label: str, tags: list[str], insights: IntentInsights | None = None) -> Intent:
    return Intent(
        id=intent_id,
        date=date,
        label=label,
        summary=f"Summary for {label}",
        start_ts=1,
        end_ts=11,
        depth=0,
        tags=tags,
        stats=IntentStats(event_count=3, duration_seconds=10),
        insights=insights or IntentInsights(),
        resume_payload=ResumePayload(),
    )


def save(store: IntentStore, intent: Intent) -> None:
    run(store.save_pipeline_run(intent.date, PipelineResult(
        intents=[intent], source_hash=intent.id, pipeline_version="v1"
    )))


def test_date_filtered_search_and_date_forgetting():
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        old = make_intent("old", "2026-07-12", "Old Terraform Work", ["project:infra"])
        target = make_intent("target", "2026-07-13", "Target Terraform Work", ["project:infra"])
        save(store, old)
        save(store, target)

        assert [row["id"] for row in run(store.search_intents("Terraform", date_from="2026-07-13", date_to="2026-07-13"))] == ["target"]
        deleted = run(store.delete_date("2026-07-13"))
        assert deleted == {"deleted_intent_ids": ["target"], "deleted_count": 1}
        assert run(store.get_intents_by_date("2026-07-13")) == []
        assert run(store.get_intent_by_id("target")) is None
        assert run(store.search_intents("Target Terraform")) == []
        assert [root.id for root in run(store.get_intents_by_date("2026-07-12"))] == ["old"]
        assert run(store.delete_date("2026-07-13")) == {"deleted_intent_ids": [], "deleted_count": 0}


def test_project_forgetting_preserves_other_projects_and_dates():
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        infra = make_intent("infra", "2026-07-13", "Infra Work", ["project:infra"])
        docs = make_intent("docs", "2026-07-14", "Docs Work", ["project:docs"])
        exact = make_intent("exact", "2026-07-14", "Exact Project Work", ["infra"])
        save(store, infra)
        run(store.save_pipeline_run("2026-07-14", PipelineResult(
            intents=[docs, exact], source_hash="docs-and-exact", pipeline_version="v1"
        )))

        deleted = run(store.delete_project("infra"))
        assert deleted["deleted_intent_ids"] == ["exact", "infra"]
        assert deleted["deleted_count"] == 2
        assert run(store.get_intent_by_id("infra")) is None
        assert run(store.get_intent_by_id("exact")) is None
        assert run(store.get_intent_by_id("docs")) is not None
        assert [row["id"] for row in run(store.search_intents("Docs Work"))] == ["docs"]


def test_fts_insights_contain_safe_aggregates_not_inserted_document_source():
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        inserted_code = "assume_role_policy = jsonencode({SECRET_DOCUMENT_SOURCE})"
        insights = IntentInsights(editor=[{"file": "iam.tf", "typed_chars": 42, "saves": 1}])
        intent = make_intent("safe", "2026-07-13", "Safe IAM Work", ["project:infra"], insights)
        save(store, intent)

        async def read_fts():
            async with store._connection() as connection:
                cursor = await connection.execute("SELECT insights, tags FROM intent_search WHERE id = ?", ("safe",))
                return await cursor.fetchone()

        row = run(read_fts())
        assert row is not None
        fts_insights = row["insights"]
        assert "iam.tf" in fts_insights
        assert inserted_code not in fts_insights
        assert "payload" not in fts_insights.lower()
        json.loads(fts_insights)
