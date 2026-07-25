"""Provider-agnostic LLM client protocol for Role B."""

from __future__ import annotations

from typing import Any, Protocol


class LLMClient(Protocol):
    """Minimal interface shared by optional provider adapters."""

    model: str

    async def respond_json(
        self,
        *,
        system: str,
        user: str,
        schema_name: str,
        schema: dict,
        max_input_chars: int | None = 4_000,
        prefer_json_object: bool = False,
    ) -> dict:
        """Return a JSON object matching the supplied schema."""

    async def respond_with_tools(
        self,
        *,
        system: str,
        messages: list[Any],
        tools: list[dict],
    ) -> dict:
        """Return output_text, tool_calls, and response_items for Copilot loops."""


ToolCallResult = dict[str, Any]
