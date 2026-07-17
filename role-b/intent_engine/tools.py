"""Safe Copilot tools.

Hard rules: use IntentStore methods for persistence; never access SQLite directly,
call Role A restore, touch the filesystem or Git, or fetch raw events.
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import date as calendar_date
from typing import Any

from intent_engine.current import CurrentIntentEngine
from intent_engine.store import IntentStore


@dataclass
class ToolContext:
    store: IntentStore
    current_engine: CurrentIntentEngine | None = None
    max_tool_calls: int = 8
    max_results: int = 10
    max_query_chars: int = 200


class ToolRegistry:
    ALLOWED_TOOLS = (
        "search_intents",
        "get_intent",
        "get_resume_payload",
        "get_current_intent",
        "get_intent_stats",
    )

    def __init__(self, context: ToolContext) -> None:
        self.context = context
        self._call_count = 0

    async def begin_request(self) -> None:
        """Reset the tool-call budget for a new Copilot request session."""

        self._call_count = 0

    def openai_tool_schemas(self) -> list[dict]:
        return [
            self._schema("search_intents", "Search stored intent summaries.", {
                "type": "object", "properties": {
                    "query": {"type": "string", "maxLength": self.context.max_query_chars},
                    "limit": {"type": "integer", "minimum": 1, "maximum": self.context.max_results},
                    "date_from": {"type": "string"}, "date_to": {"type": "string"},
                }, "required": ["query"], "additionalProperties": False,
            }),
            self._schema("get_intent", "Retrieve one stored intent.", self._id_parameters()),
            self._schema("get_resume_payload", "Retrieve stored resume context for one intent.", self._id_parameters()),
            self._schema("get_current_intent", "Retrieve the current inferred intent.", {"type": "object", "properties": {}, "additionalProperties": False}),
            self._schema("get_intent_stats", "Aggregate stored intent statistics.", {
                "type": "object", "properties": {
                    "date_from": {"type": "string"}, "date_to": {"type": "string"}, "project": {"type": "string"},
                }, "required": ["date_from", "date_to"], "additionalProperties": False,
            }),
        ]

    async def execute(self, name: str, arguments: dict) -> dict:
        self._call_count += 1
        if self._call_count > self.context.max_tool_calls:
            return {"error": "tool call limit exceeded", "code": "tool_call_cap"}
        if name not in self.ALLOWED_TOOLS:
            return self._invalid("unknown tool name")
        if not isinstance(arguments, dict):
            return self._invalid("arguments must be an object")
        try:
            if name == "search_intents":
                query = self._string(arguments, "query", required=True)
                if len(query) > self.context.max_query_chars:
                    return self._invalid("query exceeds maximum length")
                limit = self._int(arguments, "limit", default=self.context.max_results)
                if not 1 <= limit <= self.context.max_results:
                    return self._invalid(f"limit must be between 1 and {self.context.max_results}")
                date_from = self._optional_date(arguments, "date_from")
                date_to = self._optional_date(arguments, "date_to")
                if date_from and date_to and date_from > date_to:
                    return self._invalid("date_from must not be later than date_to")
                return {"results": await self.context.store.search_intents(
                    query, limit, date_from=date_from, date_to=date_to
                )}
            if name == "get_intent":
                intent = await self.context.store.get_intent_by_id(self._intent_id(arguments))
                return {"intent": self._safe_dump(intent)} if intent else {"error": "not_found"}
            if name == "get_resume_payload":
                intent_id = self._intent_id(arguments)
                intent = await self.context.store.get_intent_by_id(intent_id)
                return {"intent_id": intent_id, "resume_payload": intent.resume_payload.model_dump(mode="json")} if intent else {"error": "not_found"}
            if name == "get_current_intent":
                current = await self.context.current_engine.get_current() if self.context.current_engine else None
                return {"current_intent": current.model_dump(mode="json") if current else None}
            stats = await self.context.store.get_intent_stats(
                self._required_date(arguments, "date_from"),
                self._required_date(arguments, "date_to"),
                self._optional_string(arguments, "project"),
            )
            return {"stats": stats}
        except (TypeError, ValueError, KeyError) as exc:
            return self._invalid(str(exc))
        except Exception:
            return {"error": "tool execution failed", "code": "tool_error"}

    @staticmethod
    def _schema(name: str, description: str, parameters: dict) -> dict:
        return {"type": "function", "name": name, "description": description, "parameters": parameters}

    @staticmethod
    def _id_parameters() -> dict:
        return {"type": "object", "properties": {"intent_id": {"type": "string", "minLength": 1, "maxLength": 128}}, "required": ["intent_id"], "additionalProperties": False}

    @staticmethod
    def _invalid(message: str) -> dict:
        return {"error": message, "code": "invalid_args"}

    @staticmethod
    def _string(arguments: dict, key: str, required: bool = False) -> str:
        value = arguments.get(key)
        if value is None and not required:
            return ""
        if not isinstance(value, str) or not value.strip():
            raise ValueError(f"{key} must be a non-empty string")
        return value.strip()

    def _intent_id(self, arguments: dict) -> str:
        value = self._string(arguments, "intent_id", required=True)
        if len(value) > 128:
            raise ValueError("intent_id must be at most 128 characters")
        return value

    @staticmethod
    def _validate_date(value: str, key: str) -> str:
        try:
            parsed = calendar_date.fromisoformat(value)
        except ValueError as exc:
            raise ValueError(f"{key} must be a real YYYY-MM-DD date") from exc
        if parsed.isoformat() != value:
            raise ValueError(f"{key} must be a real YYYY-MM-DD date")
        return value

    def _optional_date(self, arguments: dict, key: str) -> str | None:
        value = self._optional_string(arguments, key)
        return self._validate_date(value, key) if value is not None else None

    def _required_date(self, arguments: dict, key: str) -> str:
        value = self._string(arguments, key, required=True)
        return self._validate_date(value, key)

    @staticmethod
    def _safe_dump(model: Any) -> dict:
        data = model.model_dump(mode="json")
        for key in ("raw", "payload", "content", "document"):
            data.pop(key, None)
        return data

    def _optional_string(self, arguments: dict, key: str) -> str | None:
        if key not in arguments or arguments[key] is None:
            return None
        return self._string(arguments, key, required=True)

    @staticmethod
    def _int(arguments: dict, key: str, default: int) -> int:
        value = arguments.get(key, default)
        if isinstance(value, bool):
            raise ValueError(f"{key} must be an integer")
        try:
            return int(value)
        except (TypeError, ValueError) as exc:
            raise ValueError(f"{key} must be an integer") from exc
