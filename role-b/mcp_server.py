"""Optional MCP transport for Role B's read-only Copilot tools."""

from __future__ import annotations

import os
from datetime import date as calendar_date, timedelta
from typing import Any

from intent_engine.current import CurrentIntentEngine
from intent_engine.store import IntentStore
from intent_engine.tools import ToolContext, ToolRegistry

TOOL_NAMES = (
    "search_intents", "get_intent", "get_resume_payload",
    "get_current_intent", "get_intent_stats", "daily_digest", "get_intent_context",
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
    server = FastMCP("Intent - Role B")

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

    @server.tool()
    async def daily_digest(date: str | None = None) -> dict:
        target_date = date or (calendar_date.today() - timedelta(days=1)).isoformat()
        intents = await registry.get_safe_intents_by_date(target_date)
        return _safe_daily_digest(intents, target_date)

    @server.tool()
    async def get_intent_context(intent_id: str) -> dict:
        result = await registry.execute("get_intent", {"intent_id": intent_id})
        intent = result.get("intent") if isinstance(result, dict) else None
        if not isinstance(intent, dict):
            return {"intent_id": intent_id, "markdown": ""}
        return {"intent_id": intent_id, "markdown": _safe_intent_context(intent)}

    server.registry = registry
    server.role_b_tool_names = TOOL_NAMES
    return server


def _safe_intent_context(intent: dict) -> str:
    """Render the ToolRegistry's safe projection without restore details."""

    label = str(intent.get("label", "Stored intent")).strip() or "Stored intent"
    summary = str(intent.get("summary", "No summary stored.")).strip() or "No summary stored."
    stats = intent.get("stats") if isinstance(intent.get("stats"), dict) else {}
    lines = [f"# {label}", "", summary, "", "## Activity"]
    lines.append(f"- Events: {stats.get('event_count', 0)}")
    lines.append(f"- Duration seconds: {stats.get('duration_seconds', 0)}")
    insights = intent.get("insights") if isinstance(intent.get("insights"), dict) else {}
    shell = insights.get("shell") if isinstance(insights.get("shell"), list) else []
    for item in shell[:3]:
        if not isinstance(item, dict):
            continue
        family = item.get("command_family")
        count = item.get("count")
        if isinstance(family, str) and isinstance(count, int) and count > 0:
            lines.append(f"- Failed {family} commands: {count}")
    return "\n".join(lines) + "\n"


def _safe_daily_digest(intents: list[dict], date: str) -> dict:
    """Summarize only the ToolRegistry's safe root-intent projection."""

    if not intents:
        return {
            "date": date,
            "headline": "No recorded work",
            "summary": "No intents were stored for this date.",
            "top_intent_ids": [],
            "intent_count": 0,
            "total_duration_seconds": 0,
        }
    primary = max(
        intents,
        key=lambda intent: (
            len(intent.get("children", [])) if isinstance(intent.get("children"), list) else 0,
            _safe_duration(intent),
        ),
    )
    return {
        "date": date,
        "headline": str(primary.get("label", "Recorded Work")),
        "summary": str(primary.get("summary", "Recorded work activity.")),
        "top_intent_ids": [str(intent["id"]) for intent in intents[:3] if isinstance(intent.get("id"), str)],
        "intent_count": len(intents),
        "total_duration_seconds": sum(_safe_duration(intent) for intent in intents),
    }


def _safe_duration(intent: dict) -> int:
    stats = intent.get("stats")
    value = stats.get("duration_seconds") if isinstance(stats, dict) else 0
    return value if isinstance(value, int) and not isinstance(value, bool) and value >= 0 else 0


def main() -> None:
    try:
        server = create_mcp_server()
    except RuntimeError as exc:
        raise SystemExit(str(exc)) from exc
    server.run()


if __name__ == "__main__":
    main()
