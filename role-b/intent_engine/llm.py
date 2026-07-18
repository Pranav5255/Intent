"""Optional OpenAI Responses API helpers used by Role B."""

from __future__ import annotations

import asyncio
import importlib
import json
import os
import re
from typing import Any


class LLMError(RuntimeError):
    """Safe, provider-agnostic error raised for LLM request failures."""


def redact_for_prompt(text: str) -> str:
    """Remove credential-like values and bound text before provider submission."""

    value = str(text or "")
    patterns = (
        (r"\bsk-[A-Za-z0-9_-]+", "[REDACTED_KEY]"),
        (r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]+", "Bearer [REDACTED_TOKEN]"),
        (r"(?i)\b(password|passwd|pwd)\s*=\s*[^\s,;]+", r"\1=[REDACTED]"),
        (r"(?i)\b(api[_-]?key|secret|token)\s*[:=]\s*[^\s,;]+", r"\1=[REDACTED]"),
    )
    for pattern, replacement in patterns:
        value = re.sub(pattern, replacement, value)
    return value[:4000]


class OpenAIResponsesClient:
    def __init__(self, api_key: str | None = None, model: str | None = None, timeout_seconds: float = 20.0) -> None:
        self.api_key = api_key or os.environ.get("OPENAI_API_KEY")
        if not self.api_key:
            raise ValueError("OPENAI_API_KEY is required for live LLM use")
        self.model = model or os.environ.get("INTENT_OS_LLM_MODEL", "gpt-4o-mini")
        self.timeout_seconds = timeout_seconds
        self._client: Any | None = None

    def _get_client(self) -> Any:
        if self._client is not None:
            return self._client
        try:
            module = importlib.import_module("openai")
            self._client = module.AsyncOpenAI(api_key=self.api_key, timeout=self.timeout_seconds)
            return self._client
        except ImportError as exc:
            raise LLMError("Optional OpenAI SDK is unavailable; install requirements-openai.txt") from exc
        except Exception as exc:
            raise LLMError("Unable to initialize the OpenAI client") from exc

    async def respond_json(self, *, system: str, user: str, schema_name: str, schema: dict) -> dict:
        async def request() -> Any:
            client = self._get_client()
            return await client.responses.create(
                model=self.model,
                instructions=redact_for_prompt(system),
                input=redact_for_prompt(user),
                text={"format": {"type": "json_schema", "name": schema_name, "schema": schema, "strict": True}},
            )

        try:
            response = await asyncio.wait_for(request(), timeout=self.timeout_seconds)
            text = self._output_text(response)
            parsed = json.loads(text)
            if not isinstance(parsed, dict):
                raise ValueError("structured response must be an object")
            return parsed
        except LLMError:
            raise
        except asyncio.TimeoutError as exc:
            raise LLMError("LLM request timed out") from exc
        except Exception as exc:
            raise LLMError("LLM request or JSON parsing failed") from exc

    async def respond_with_tools(self, *, system: str, messages: list[dict], tools: list[dict]) -> dict:
        async def request() -> Any:
            client = self._get_client()
            safe_input = []
            for message in messages:
                # A continued Responses API request must include the exact output
                # items from the preceding response (not Chat Completions-style
                # assistant/tool messages). Those SDK objects are already safe
                # provider output, so only redact caller-supplied dictionaries.
                if isinstance(message, dict):
                    safe_input.append({
                        key: redact_for_prompt(value) if isinstance(value, str) else value
                        for key, value in message.items()
                    })
                else:
                    safe_input.append(message)
            return await client.responses.create(
                model=self.model,
                instructions=redact_for_prompt(system),
                input=safe_input,
                tools=tools,
            )

        try:
            response = await asyncio.wait_for(request(), timeout=self.timeout_seconds)
            calls = []
            for item in self._output_items(response):
                item_type = self._get(item, "type")
                if item_type in {"function_call", "tool_call"}:
                    arguments = self._get(item, "arguments", "{}")
                    try:
                        arguments = json.loads(arguments) if isinstance(arguments, str) else arguments
                    except json.JSONDecodeError as exc:
                        raise LLMError("LLM tool arguments were not valid JSON") from exc
                    calls.append({"name": self._get(item, "name", ""), "arguments": arguments, "call_id": self._get(item, "call_id", "")})
            return {
                "output_text": self._output_text(response, required=False),
                "tool_calls": calls,
                # The caller must return these exact items on the next Responses
                # request before supplying function_call_output records.
                "response_items": self._output_items(response),
            }
        except LLMError:
            raise
        except asyncio.TimeoutError as exc:
            raise LLMError("LLM request timed out") from exc
        except Exception as exc:
            raise LLMError("LLM tool request failed") from exc

    @classmethod
    def _output_text(cls, response: Any, required: bool = True) -> str | None:
        text = cls._get(response, "output_text")
        if isinstance(text, str) and text:
            return text
        for item in cls._output_items(response):
            for content in cls._get(item, "content", []) or []:
                candidate = cls._get(content, "text")
                if isinstance(candidate, str) and candidate:
                    return candidate
        if required:
            raise ValueError("LLM response did not contain output text")
        return None

    @staticmethod
    def _output_items(response: Any) -> list[Any]:
        output = OpenAIResponsesClient._get(response, "output", [])
        return output if isinstance(output, list) else []

    @staticmethod
    def _get(value: Any, key: str, default: Any = None) -> Any:
        if isinstance(value, dict):
            return value.get(key, default)
        return getattr(value, key, default)
