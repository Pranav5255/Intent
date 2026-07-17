"""Optional MCP transport for Role B's read-only Copilot tools."""

from __future__ import annotations

import os
from typing import Any

from intent_engine.current import CurrentIntentEngine
from intent_engine.store import IntentStore
from intent_engine.tools import ToolContext, ToolRegistry

TOOL_NAMES = (
    "search_intents", "get_intent", "get_resume_payload",
    "get_current_intent", "get_intent_stats",
)


def _load_fastmcp() -> type:
    try:
        from mcp.server.fastmcp import FastMCP
    except ImportError as exc:  # pragma: no cover
        raise RuntimeError(
            "MCP support is optional. Install it with pip install -r requirements-mcp.txt"
        ) from exc
    return FastMCP


def create_mcp_server(store: IntentStore | None = None, current_engine: CurrentIntentEngine | None = None) -> Any:
    """Construct an MCP server backed entirely by the existing ToolRegistry."""
    FastMCP = _load_fastmcp()
    registry = ToolRegistry(ToolContext(
        store or IntentStore(os.environ.get("ROLE_B_DB_PATH", "intents.db")),
        current_engine,
    ))
    server = FastMCP("Intent OS - Role B")

    @server.tool()
    async def search_intents(query: str, limit: int = 10, date_from: str | None = None, date_to: str | None = None) -> dict:
        return await registry.execute("search_intents", {"query": query, "limit": limit, "date_from": date_from, "date_to": date_to})

    @server.tool()
    async def get_intent(intent_id: str) -> dict:
        return await registry.execute("get_intent", {"intent_id": intent_id})

    @server.tool()
    async def get_resume_payload(intent_id: str) -> dict:
        return await registry.execute("get_resume_payload", {"intent_id": intent_id})

    @server.tool()
    async def get_current_intent() -> dict:
        return await registry.execute("get_current_intent", {})

    @server.tool()
    async def get_intent_stats(date_from: str, date_to: str, project: str | None = None) -> dict:
        return await registry.execute("get_intent_stats", {"date_from": date_from, "date_to": date_to, "project": project})

    server.registry = registry
    server.role_b_tool_names = TOOL_NAMES
    return server


def main() -> None:
    try:
        server = create_mcp_server()
    except RuntimeError as exc:
        raise SystemExit(str(exc)) from exc
    server.run()


if __name__ == "__main__":
    main()
