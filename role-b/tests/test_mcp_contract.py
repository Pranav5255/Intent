import asyncio
import json
import tempfile
from pathlib import Path

import pytest

import mcp_server
from intent_engine.schemas import Intent, IntentInsights, IntentStats, PipelineResult, ResumePayload
from intent_engine.store import IntentStore


def _seed():
    temp = tempfile.TemporaryDirectory()
    store = IntentStore(str(Path(temp.name) / "intents.db"))
    intent = Intent(
        id="mcp-1", date="2026-07-13", label="IAM Work", summary="Reviewed IAM policy.",
        start_ts=1, end_ts=5, depth=0,
        stats=IntentStats(event_count=2, duration_seconds=4),
        insights=IntentInsights(), resume_payload=ResumePayload(files=["/repo/iam.tf"]),
    )
    asyncio.run(store.save_pipeline_run("2026-07-13", PipelineResult(intents=[intent], source_hash="mcp", pipeline_version="1")))
    return temp, store


def _real_handler(server, name):
    """Support FastMCP's current registered-tool metadata without coupling production code to it."""
    manager = getattr(server, "_tool_manager", None)
    tools = getattr(manager, "_tools", None)
    if isinstance(tools, dict) and name in tools:
        tool = tools[name]
        return getattr(tool, "fn", tool)
    tools = getattr(server, "_tools", None)
    if isinstance(tools, dict) and name in tools:
        tool = tools[name]
        return getattr(tool, "fn", tool)
    pytest.skip("MCP SDK does not expose registered handler metadata")


def test_mcp_handlers_match_registry_when_sdk_is_installed():
    pytest.importorskip("mcp")
    temp, store = _seed()
    try:
        server = mcp_server.create_mcp_server(store)
        arguments = {
            "search_intents": {"query": "IAM", "limit": 10},
            "get_intent": {"intent_id": "mcp-1"},
            "get_resume_payload": {"intent_id": "mcp-1"},
            "get_current_intent": {},
            "get_intent_stats": {"date_from": "2026-07-13", "date_to": "2026-07-13"},
        }
        for name in server.role_b_tool_names:
            asyncio.run(server.registry.begin_request())
            expected = asyncio.run(server.registry.execute(name, arguments[name]))
            handler = _real_handler(server, name)
            asyncio.run(server.registry.begin_request())
            actual = asyncio.run(handler(**arguments[name]))
            assert actual == expected
            json.dumps(actual)
    finally:
        temp.cleanup()


def test_unknown_tool_is_rejected_and_not_exposed():
    temp, store = _seed()
    try:
        # This assertion remains runnable without the optional MCP SDK.
        from intent_engine.tools import ToolContext, ToolRegistry
        result = asyncio.run(ToolRegistry(ToolContext(store)).execute("unknown_tool", {}))
        assert result["code"] == "invalid_args"
        if hasattr(mcp_server, "TOOL_NAMES"):
            assert "unknown_tool" not in mcp_server.TOOL_NAMES
    finally:
        temp.cleanup()


def test_adapter_contains_no_sql_or_direct_async_database_access():
    source = Path(mcp_server.__file__).read_text(encoding="utf-8").lower()
    assert "aiosqlite.connect" not in source
    for statement in ("select ", "insert ", "delete ", "create table"):
        assert statement not in source
