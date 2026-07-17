import asyncio
import tempfile
from pathlib import Path

from intent_engine.schemas import Intent, IntentInsights, IntentStats, PipelineResult, ResumePayload
from intent_engine.store import IntentStore
from intent_engine.tools import ToolContext, ToolRegistry


def run(awaitable):
    return asyncio.run(awaitable)


def seed(store):
    intent = Intent(
        id="tool-intent", date="2026-07-13", label="Tool Work", summary="Tool summary",
        start_ts=1, end_ts=10, depth=0, tags=["project:infra"],
        stats=IntentStats(event_count=2, duration_seconds=9), insights=IntentInsights(), resume_payload=ResumePayload(files=["/repo/a.py"]),
    )
    run(store.save_pipeline_run("2026-07-13", PipelineResult(intents=[intent], source_hash="tool", pipeline_version="v1")))


def test_unknown_and_malformed_tools_return_structured_errors():
    with tempfile.TemporaryDirectory() as directory:
        registry = ToolRegistry(ToolContext(IntentStore(str(Path(directory) / "intents.db"))))
        assert run(registry.execute("delete_everything", {}))["code"] == "invalid_args"
        assert run(registry.execute("search_intents", []))["code"] == "invalid_args"
        assert run(registry.execute("search_intents", {"query": "x" * 201}))["code"] == "invalid_args"


def test_allowlisted_schemas_and_json_safe_store_tools():
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        seed(store)
        registry = ToolRegistry(ToolContext(store))
        assert {schema["name"] for schema in registry.openai_tool_schemas()} == set(ToolRegistry.ALLOWED_TOOLS)
        assert run(registry.execute("search_intents", {"query": "Tool"}))["results"][0]["id"] == "tool-intent"
        assert run(registry.execute("get_intent", {"intent_id": "tool-intent"}))["intent"]["id"] == "tool-intent"
        assert run(registry.execute("get_resume_payload", {"intent_id": "tool-intent"}))["resume_payload"]["files"] == ["/repo/a.py"]
        assert run(registry.execute("get_current_intent", {})) == {"current_intent": None}
        stats = run(registry.execute("get_intent_stats", {"date_from": "2026-07-13", "date_to": "2026-07-13"}))
        assert stats["stats"]["intent_count"] == 1
