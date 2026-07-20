"""Safe Copilot tools.

Hard rules: use IntentStore methods for persistence; never access SQLite directly,
call Role A restore, touch the filesystem or Git, or fetch raw events.
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import date as calendar_date

from intent_engine.current import CurrentIntentEngine
from intent_engine.schemas import SAFE_INTENT_PRIVACY_POLICY_VERSION, Intent, ResumePayload
from intent_engine.store import IntentStore


@dataclass
class ToolContext:
    store: IntentStore
    current_engine: CurrentIntentEngine | None = None
    max_tool_calls: int = 8
    max_results: int = 10
    max_query_chars: int = 200


class ToolRegistry:
    """Read-only model tools with a strict cloud-safe response projection."""

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
            self._schema("get_resume_payload", "Check whether restore context is available for one intent.", self._id_parameters()),
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
                records = await self.context.store.search_intents(query, limit, date_from=date_from, date_to=date_to)
                safe_records = []
                for record in records:
                    intent_id = record.get("id") if isinstance(record, dict) else None
                    intent = await self.context.store.get_intent_by_id(intent_id) if isinstance(intent_id, str) else None
                    safe_records.append(self._safe_search_record(record, intent))
                return {"results": safe_records}
            if name == "get_intent":
                intent = await self.context.store.get_intent_by_id(self._intent_id(arguments))
                return {"intent": self._safe_dump(intent)} if intent else {"error": "not_found"}
            if name == "get_resume_payload":
                intent_id = self._intent_id(arguments)
                intent = await self.context.store.get_intent_by_id(intent_id)
                return self._safe_resume_metadata(intent_id, intent.resume_payload) if intent else {"error": "not_found"}
            if name == "get_current_intent":
                current = await self.context.current_engine.get_current() if self.context.current_engine else None
                return {"current_intent": self._safe_current_intent(current) if current else None}
            stats = await self.context.store.get_intent_stats(
                self._required_date(arguments, "date_from"),
                self._required_date(arguments, "date_to"),
                self._optional_string(arguments, "project"),
            )
            return {"stats": self._safe_stats(stats)}
        except (TypeError, ValueError, KeyError) as exc:
            return self._invalid(str(exc))
        except Exception:
            return {"error": "tool execution failed", "code": "tool_error"}

    async def get_resume_payload_for_response(self, intent_id: str) -> ResumePayload | None:
        """Fetch exact restore data for local response assembly, never for model input."""

        intent = await self.context.store.get_intent_by_id(intent_id)
        return intent.resume_payload.model_copy(deep=True) if intent else None

    async def get_safe_intents_by_date(self, date: str) -> list[dict]:
        """Return model-safe root intents for adapter-only aggregate responses."""

        return [self._safe_dump(intent) for intent in await self.context.store.get_intents_by_date(date)]

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

    @classmethod
    def _safe_dump(cls, intent: Intent) -> dict:
        """Allowlist the stable, non-raw intent fields exposed to a model."""

        label, summary = cls._safe_public_text(intent)
        return {
            "id": intent.id,
            "parent_id": intent.parent_id,
            "date": intent.date,
            "label": label,
            "summary": summary,
            "confidence": intent.confidence,
            "start_ts": intent.start_ts,
            "end_ts": intent.end_ts,
            "depth": intent.depth,
            "stats": {
                "event_count": intent.stats.event_count,
                "duration_seconds": intent.stats.duration_seconds,
                "activity_families": cls._safe_source_counts(intent.stats.sources),
            },
            "insights": cls._safe_insights(intent),
            "todo_count": len(intent.todos),
            "semantic": cls._safe_semantic(intent),
            "children": [cls._safe_dump(child) for child in intent.children],
        }

    @classmethod
    def _safe_search_record(cls, record: object, intent: Intent | None) -> dict:
        if not isinstance(record, dict):
            return {}
        safe = {
            key: record[key]
            for key in ("id", "date")
            if isinstance(record.get(key), str)
        }
        if intent is not None:
            label, summary = cls._safe_public_text(intent)
            safe.update({"label": label, "summary": summary})
        return safe

    @staticmethod
    def _safe_public_text(intent: Intent) -> tuple[str, str]:
        if intent.privacy_policy_version == SAFE_INTENT_PRIVACY_POLICY_VERSION:
            return intent.label, intent.summary
        return "Stored Work", "A legacy stored intent is available locally."

    @staticmethod
    def _safe_resume_metadata(intent_id: str, payload: ResumePayload) -> dict:
        return {
            "intent_id": intent_id,
            "resume_payload_available": True,
            "resume_context": {
                "file_count": len(payload.files),
                "url_count": len(payload.urls),
                "has_shell_context": bool(payload.shell),
            },
        }

    @staticmethod
    def _safe_current_intent(current) -> dict:
        return {
            "label": current.label,
            "summary": current.summary,
            "confidence": current.confidence,
            "since_ts": current.since_ts,
        }

    @staticmethod
    def _safe_stats(stats: object) -> dict:
        if not isinstance(stats, dict):
            return {}
        by_date = []
        for item in stats.get("by_date", []):
            if not isinstance(item, dict) or not isinstance(item.get("date"), str):
                continue
            by_date.append({
                "date": item["date"],
                "intent_count": ToolRegistry._non_negative_int(item.get("intent_count")),
                "duration_seconds": ToolRegistry._non_negative_int(item.get("duration_seconds")),
            })
        return {
            "date_from": stats.get("date_from") if isinstance(stats.get("date_from"), str) else None,
            "date_to": stats.get("date_to") if isinstance(stats.get("date_to"), str) else None,
            "intent_count": ToolRegistry._non_negative_int(stats.get("intent_count")),
            "total_duration_seconds": ToolRegistry._non_negative_int(stats.get("total_duration_seconds")),
            "event_count": ToolRegistry._non_negative_int(stats.get("event_count")),
            "by_date": by_date,
        }

    @staticmethod
    def _safe_insights(intent: Intent) -> dict:
        editor = []
        browser = []
        shell = []
        for item in intent.insights.editor:
            if isinstance(item, dict):
                editor.append({
                    "typed_chars": ToolRegistry._non_negative_int(item.get("typed_chars")),
                    "saves": ToolRegistry._non_negative_int(item.get("saves")),
                })
        for item in intent.insights.browser:
            if isinstance(item, dict):
                browser.append({"visits": ToolRegistry._non_negative_int(item.get("visits"))})
        for item in intent.insights.shell:
            if not isinstance(item, dict):
                continue
            family = item.get("command_family")
            if family not in {"terraform", "git", "pytest", "python", "npm", "pip", "docker", "make", "unknown"}:
                family = "unknown"
            shell.append({
                "command_family": family,
                "exit_code": item.get("exit_code") if isinstance(item.get("exit_code"), int) and not isinstance(item.get("exit_code"), bool) else None,
                "count": ToolRegistry._non_negative_int(item.get("count")),
            })
        return {"editor": editor, "browser": browser, "shell": shell}

    @staticmethod
    def _safe_semantic(intent: Intent) -> dict | None:
        if intent.semantic is None:
            return None
        roles = {
            role: ToolRegistry._non_negative_int(count)
            for role, count in intent.semantic.event_roles.items()
            if role in {"task", "supporting_context", "background", "unrelated"}
        }
        return {
            "refined": intent.semantic.refined,
            "confidence": intent.semantic.confidence,
            "event_roles": roles,
        }

    @staticmethod
    def _safe_source_counts(sources: dict[str, int]) -> dict[str, int]:
        families = {
            "vscode": "editor",
            "firefox": "browser",
            "chrome": "browser",
            "shell": "command",
            "linux": "focus",
            "filesystem": "file_change",
        }
        counts: dict[str, int] = {}
        for source, count in sources.items():
            family = families.get(source, "other")
            safe_count = ToolRegistry._non_negative_int(count)
            if safe_count:
                counts[family] = counts.get(family, 0) + safe_count
        return counts

    @staticmethod
    def _non_negative_int(value: object) -> int:
        return value if isinstance(value, int) and not isinstance(value, bool) and value >= 0 else 0

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
