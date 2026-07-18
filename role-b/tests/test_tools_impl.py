import asyncio
import tempfile
from pathlib import Path

from intent_engine.schemas import Intent, IntentInsights, IntentStats, PipelineResult, ResumePayload
from intent_engine.store import IntentStore
from intent_engine.tools import ToolContext, ToolRegistry


def run(awaitable):
    return asyncio.run(awaitable)


def setup_registry():
    directory = tempfile.TemporaryDirectory()
    store = IntentStore(str(Path(directory.name) / "intents.db"))
    intent = Intent(
        id="impl-intent", date="2026-07-13", label="Implementation Work", summary="Implementation summary",
        start_ts=1, end_ts=20, depth=0, tags=["project:infra"], stats=IntentStats(event_count=4, duration_seconds=19),
        insights=IntentInsights(), resume_payload=ResumePayload(files=["/repo/iam.tf"]),
    )
    run(store.save_pipeline_run("2026-07-13", PipelineResult(intents=[intent], source_hash="impl", pipeline_version="v1")))
    return directory, ToolRegistry(ToolContext(store, max_results=2, max_query_chars=20))


def test_all_store_backed_tools_and_not_found():
    directory, registry = setup_registry()
    try:
        search = run(registry.execute("search_intents", {"query": "Implementation", "date_from": "2026-07-13", "date_to": "2026-07-13", "limit": 1}))
        assert set(search["results"][0]) == {"id", "label", "summary", "date", "highlight_snippet"}
        assert run(registry.execute("get_intent", {"intent_id": "impl-intent"}))["intent"]["id"] == "impl-intent"
        assert run(registry.execute("get_resume_payload", {"intent_id": "impl-intent"}))["resume_payload"]["files"] == ["/repo/iam.tf"]
        assert run(registry.execute("get_intent_stats", {"date_from": "2026-07-13", "date_to": "2026-07-13"}))["stats"]["event_count"] == 4
        assert run(registry.execute("get_intent", {"intent_id": "missing"})) == {"error": "not_found"}
        assert run(registry.execute("get_resume_payload", {"intent_id": "missing"})) == {"error": "not_found"}
    finally:
        directory.cleanup()


def test_argument_validation_and_current_null():
    directory, registry = setup_registry()
    try:
        assert run(registry.execute("get_intent", {"intent_id": "x" * 129}))["code"] == "invalid_args"
        assert run(registry.execute("search_intents", {"query": ""}))["code"] == "invalid_args"
        assert run(registry.execute("search_intents", {"query": "x", "limit": 3}))["code"] == "invalid_args"
        assert run(registry.execute("search_intents", {"query": "x", "date_from": "2026-02-30"}))["code"] == "invalid_args"
        assert run(registry.execute("get_current_intent", {})) == {"current_intent": None}
    finally:
        directory.cleanup()


def test_tools_module_has_no_sqlite_imports():
    source = Path(__file__).parents[1].joinpath("intent_engine", "tools.py").read_text(encoding="utf-8")
    assert "sqlite3" not in source
    assert "aiosqlite" not in source
