"""Provider-agnostic LLM client protocol for Role B."""

from __future__ import annotations

from typing import Any, Protocol


class LLMClient(Protocol):
    """Minimal interface shared by OpenAI and Gemini adapters."""

    model: str

    async def respond_json(
        self,
        *,
        system: str,
        user: str,
        schema_name: str,
        schema: dict,
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
