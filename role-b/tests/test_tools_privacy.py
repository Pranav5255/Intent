import asyncio
import tempfile
from pathlib import Path

from intent_engine.schemas import ContextEvidence, Intent, IntentInsights, IntentStats, PipelineResult, ResumePayload
from intent_engine.store import IntentStore
from intent_engine.tools import ToolContext, ToolRegistry


def run(awaitable):
    return asyncio.run(awaitable)


def registry(max_tool_calls=8, max_results=2, max_query_chars=20):
    directory = tempfile.TemporaryDirectory()
    store = IntentStore(str(Path(directory.name) / "intents.db"))
    intent = Intent(
        id="privacy-intent", date="2026-07-13", label="Safe Work", summary="Safe summary",
        start_ts=1, end_ts=10, depth=0, tags=["project:infra"],
        stats=IntentStats(event_count=1, duration_seconds=9, sources={"private-app-name": 1}, unique_apps=["private-app-name"]),
        insights=IntentInsights(
            editor=[{"file": "private-module.py", "typed_chars": 42, "saves": 1}],
            browser=[{"domain": "secret.example.test", "visits": 2}],
            shell=[{"command_family": "terraform", "exit_code": 1, "count": 1}],
        ),
        evidence=[ContextEvidence(field="document_change.text", value="SECRET_RAW_EVENT_SOURCE")],
        resume_payload=ResumePayload(
            files=["/private/workspace/private-module.py"],
            urls=["https://secret.example.test/restore"],
            shell={"cwd": "/private/workspace", "last_cmd": "SECRET_SHELL_COMMAND"},
        ),
    )
    run(store.save_pipeline_run("2026-07-13", PipelineResult(intents=[intent], source_hash="privacy", pipeline_version="v1")))
    return directory, ToolRegistry(ToolContext(store, max_tool_calls=max_tool_calls, max_results=max_results, max_query_chars=max_query_chars))


def test_tool_call_cap_and_request_reset():
    directory, tools = registry(max_tool_calls=2)
    try:
        assert run(tools.execute("get_current_intent", {})) == {"current_intent": None}
        assert run(tools.execute("get_current_intent", {})) == {"current_intent": None}
        assert run(tools.execute("get_current_intent", {}))["code"] == "tool_call_cap"
        run(tools.begin_request())
        assert run(tools.execute("get_current_intent", {})) == {"current_intent": None}
    finally:
        directory.cleanup()


def test_privacy_shapes_and_argument_policy():
    directory, tools = registry()
    try:
        def keys(value):
            if isinstance(value, dict):
                return {str(key).lower() for key in value} | set().union(*(keys(item) for item in value.values()))
            if isinstance(value, list):
                return set().union(*(keys(item) for item in value)) if value else set()
            return set()

        assert not ({"raw", "changes", "text"} & keys(tools.openai_tool_schemas()))
        result = run(tools.execute("get_intent", {"intent_id": "privacy-intent"}))
        assert not ({"raw", "changes", "text"} & keys(result))
        serialized = str(result)
        for marker in (
            "SECRET_RAW_EVENT_SOURCE", "private-module.py", "secret.example.test",
            "/private/workspace", "SECRET_SHELL_COMMAND", "private-app-name",
        ):
            assert marker not in serialized
        assert "resume_payload" not in result["intent"]
        assert "evidence" not in result["intent"]
        assert result["intent"]["label"] == "Stored Work"
        assert result["intent"]["summary"] == "A legacy stored intent is available locally."
        resume = run(tools.execute("get_resume_payload", {"intent_id": "privacy-intent"}))
        assert resume == {
            "intent_id": "privacy-intent",
            "resume_payload_available": True,
            "resume_context": {"file_count": 1, "url_count": 1, "has_shell_context": True},
        }
        search = run(tools.execute("search_intents", {"query": "Safe"}))
        assert set(search["results"][0]) == {"id", "label", "summary", "date"}
        assert search["results"][0]["label"] == "Stored Work"
        assert run(tools.execute("search_intents", {"query": "x" * 21}))["code"] == "invalid_args"
        # Limits above max_results are rejected, not clamped.
        assert run(tools.execute("search_intents", {"query": "Safe", "limit": 3}))["code"] == "invalid_args"
    finally:
        directory.cleanup()
