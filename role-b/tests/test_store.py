from __future__ import annotations

import asyncio
import sys
import tempfile
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parents[1]))

from intent_engine.schemas import Intent, IntentInsights, IntentStats, PipelineResult, ResumePayload
from intent_engine.store import IntentStore


def intent(intent_id: str, *, date: str = "2026-07-13", parent_id: str | None = None, depth: int = 0, label: str = "Deploy infrastructure", children: list[Intent] | None = None) -> Intent:
    return Intent(
        id=intent_id,
        parent_id=parent_id,
        date=date,
        label=label,
        summary=f"Summary for {label}",
        start_ts=1 if depth == 0 else 2,
        end_ts=10,
        depth=depth,
        stats=IntentStats(event_count=1, duration_seconds=9),
        insights=IntentInsights(),
        resume_payload=ResumePayload(),
        children=children or [],
    )


def run(coroutine):
    return asyncio.run(coroutine)


def test_save_cache_and_rebuild_children() -> None:
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        child = intent("child", parent_id="root", depth=1, label="Run Terraform")
        root = intent("root", children=[child])
        result = PipelineResult(intents=[root], source_hash="hash-one", pipeline_version="v1")

        run(store.save_pipeline_run("2026-07-13", result))

        assert run(store.cache_exists("2026-07-13", "hash-one")) is True
        cached = run(store.get_cached_intents("2026-07-13", "hash-one"))
        assert cached is not None and cached[0].children[0].id == "child"
        assert run(store.get_intent_by_id("child")).children == []
        assert run(store.get_intent_by_id("root")).children[0].id == "child"


def test_cache_miss_empty_run_and_date_replacement() -> None:
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        empty = PipelineResult(source_hash="empty", pipeline_version="v1")
        run(store.save_pipeline_run("2026-07-13", empty))

        assert run(store.get_cached_intents("2026-07-13", "missing")) is None
        assert run(store.get_cached_intents("2026-07-13", "empty")) == []

        replacement = PipelineResult(intents=[intent("replacement", label="Read docs")], source_hash="new", pipeline_version="v1")
        run(store.save_pipeline_run("2026-07-13", replacement))

        assert run(store.cache_exists("2026-07-13", "empty")) is False
        assert run(store.get_intent_by_id("replacement")) is not None


def test_search_fts_and_like_fallback_are_parameterized() -> None:
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        result = PipelineResult(
            intents=[intent("terraform", label="Deploy Terraform"), intent("docs", label="Read documentation")],
            source_hash="search", pipeline_version="v1",
        )
        run(store.save_pipeline_run("2026-07-13", result))

        fts_results = run(store.search_intents("Terraform"))
        assert [item["id"] for item in fts_results] == ["terraform"]
        store._fts_available = False
        fallback_results = run(store.search_intents("Terraform"))
        assert [item["id"] for item in fallback_results] == ["terraform"]
        assert run(store.search_intents("' OR 1=1 --")) == []


def test_search_highlights_roots_and_invalidates_memory_cache() -> None:
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        root = intent("root", label="Deploy Infrastructure")
        root.summary = "Intro details " * 8 + "Terraform" + " deployment details " * 8
        child = intent("child", parent_id="root", depth=1, label="Secret Child Keyword")
        root.children = [child]
        run(store.save_pipeline_run("2026-07-13", PipelineResult(intents=[root], source_hash="first", pipeline_version="v1")))

        fts = run(store.search_intents("Terraform"))
        assert [item["id"] for item in fts] == ["root"]
        assert "**Terraform**" in fts[0]["highlight_snippet"]
        assert fts[0]["highlight_snippet"].startswith("...")
        assert fts[0]["highlight_snippet"].endswith("...")
        assert run(store.search_intents("Secret Child Keyword")) == []

        fts[0]["label"] = "mutated"
        assert run(store.search_intents("terraform"))[0]["label"] == "Deploy Infrastructure"
        store._search_cache.clear()
        store._fts_available = False
        fallback = run(store.search_intents("Terraform"))
        assert [item["id"] for item in fallback] == ["root"]

        replacement = intent("new-root", label="Fresh Work")
        replacement.summary = "Fresh summary for search cache invalidation."
        run(store.save_pipeline_run("2026-07-13", PipelineResult(intents=[replacement], source_hash="second", pipeline_version="v1")))
        assert [item["id"] for item in run(store.search_intents("Fresh"))] == ["new-root"]
        run(store.delete_date("2026-07-13"))
        assert run(store.search_intents("Fresh")) == []


def test_search_date_ranges_are_inclusive_and_cache_scoped() -> None:
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        run(store.save_pipeline_run("2026-07-12", PipelineResult(intents=[intent("old", date="2026-07-12", label="Deploy Terraform")], source_hash="old", pipeline_version="v1")))
        run(store.save_pipeline_run("2026-07-13", PipelineResult(intents=[intent("middle", date="2026-07-13", label="Deploy Terraform")], source_hash="middle", pipeline_version="v1")))
        run(store.save_pipeline_run("2026-07-14", PipelineResult(intents=[intent("new", date="2026-07-14", label="Deploy Terraform")], source_hash="new", pipeline_version="v1")))

        assert [item["id"] for item in run(store.search_intents("Terraform", date_from="2026-07-13", date_to="2026-07-14"))] == ["middle", "new"]
        assert [item["id"] for item in run(store.search_intents("Terraform", date_from="2026-07-14"))] == ["new"]
        assert [item["id"] for item in run(store.search_intents("Terraform", date_to="2026-07-12"))] == ["old"]
        with pytest.raises(ValueError):
            run(store.search_intents("Terraform", date_from="2026-02-30"))
