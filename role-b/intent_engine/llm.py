"""OpenAI-compatible Responses API helpers used by optional Role B providers."""

from __future__ import annotations

import asyncio
import importlib
import json
import os
import re
from typing import Any


class LLMError(RuntimeError):
    """Safe, provider-agnostic error raised for LLM request failures."""

    def __init__(
        self,
        message: str,
        *,
        status_code: int | None = None,
        retry_after_seconds: float | None = None,
        error_code: str | None = None,
    ) -> None:
        super().__init__(message)
        self.status_code = status_code
        self.retry_after_seconds = retry_after_seconds
        self.error_code = error_code


def redact_for_prompt(text: str, max_chars: int | None = 4_000) -> str:
    """Remove credential-like values and optionally bound provider input."""

    value = str(text or "")
    patterns = (
        (r"\bsk-[A-Za-z0-9_-]+", "[REDACTED_KEY]"),
        (r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]+", "Bearer [REDACTED_TOKEN]"),
        (r"(?i)\b(password|passwd|pwd)\s*=\s*[^\s,;]+", r"\1=[REDACTED]"),
        (r"(?i)\b(api[_-]?key|secret|token)\s*[:=]\s*[^\s,;]+", r"\1=[REDACTED]"),
    )
    for pattern, replacement in patterns:
        value = re.sub(pattern, replacement, value)
    return value if max_chars is None else value[:max_chars]


class OpenAIResponsesClient:
    """Responses API client for OpenAI and OpenAI-compatible providers."""

    def __init__(
        self,
        api_key: str | None = None,
        model: str | None = None,
        timeout_seconds: float = 60.0,
        *,
        base_url: str | None = None,
        api_key_env: str = "OPENAI_API_KEY",
        default_model: str = "gpt-4o-mini",
        provider_name: str = "OpenAI",
    ) -> None:
        raw_api_key = api_key if api_key is not None else os.environ.get(api_key_env, "")
        self.api_key = raw_api_key.strip() if isinstance(raw_api_key, str) else ""
        if not self.api_key:
            raise ValueError(f"{api_key_env} is required for live LLM use")
        self.model = model or os.environ.get("INTENT_OS_LLM_MODEL", "").strip() or default_model
        self.base_url = base_url.strip().rstrip("/") if isinstance(base_url, str) and base_url.strip() else None
        self._provider_name = provider_name
        self.timeout_seconds = timeout_seconds
        self._client: Any | None = None

    def _get_client(self) -> Any:
        if self._client is not None:
            return self._client
        try:
            module = importlib.import_module("openai")
            client_kwargs = {"api_key": self.api_key, "timeout": self.timeout_seconds}
            if self.base_url is not None:
                client_kwargs["base_url"] = self.base_url
            self._client = module.AsyncOpenAI(**client_kwargs)
            return self._client
        except ImportError as exc:
            raise LLMError("Optional OpenAI SDK is unavailable; install requirements-openai.txt") from exc
        except Exception as exc:
            raise LLMError(f"Unable to initialize the {self._provider_name} client") from exc

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
        async def request() -> Any:
            client = self._get_client()
            return await client.responses.create(
                model=self.model,
                instructions=redact_for_prompt(system),
                input=redact_for_prompt(user, max_input_chars),
                text={"format": (
                    {"type": "json_object"}
                    if prefer_json_object
                    else {"type": "json_schema", "name": schema_name, "schema": schema, "strict": True}
                )},
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
            raise _provider_error("LLM request or JSON parsing failed", exc) from exc

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
            raise _provider_error("LLM tool request failed", exc) from exc

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


def _provider_error(message: str, exc: Exception) -> LLMError:
    """Preserve safe retry metadata without copying a provider error body."""

    status_code = getattr(exc, "status_code", None)
    status_code = status_code if isinstance(status_code, int) else None
    error_code = getattr(exc, "code", None)
    error_code = error_code if isinstance(error_code, str) else None
    body = getattr(exc, "body", None)
    nested_error = body.get("error") if isinstance(body, dict) else None
    if error_code is None and isinstance(nested_error, dict):
        nested_code = nested_error.get("code")
        error_code = nested_code if isinstance(nested_code, str) else None
    headers = getattr(getattr(exc, "response", None), "headers", None)
    retry_value = None
    if headers is not None:
        retry_value = headers.get("retry-after") or headers.get("x-ratelimit-reset-tokens")
    retry_after_seconds = _retry_after_seconds(retry_value)
    return LLMError(
        message,
        status_code=status_code,
        retry_after_seconds=retry_after_seconds,
        error_code=error_code,
    )


def _retry_after_seconds(value: object) -> float | None:
    if not isinstance(value, str):
        return None
    match = re.search(r"\d+(?:\.\d+)?", value)
    return float(match.group()) if match else None


class GroqResponsesClient(OpenAIResponsesClient):
    """Groq Responses adapter using its OpenAI-compatible endpoint."""

    DEFAULT_BASE_URL = "https://api.groq.com/openai/v1"
    DEFAULT_MODEL = "openai/gpt-oss-20b"

    def __init__(
        self,
        api_key: str | None = None,
        model: str | None = None,
        timeout_seconds: float = 60.0,
        base_url: str | None = None,
    ) -> None:
        selected_base_url = base_url or os.environ.get("GROQ_BASE_URL") or self.DEFAULT_BASE_URL
        super().__init__(
            api_key=api_key,
            model=model,
            timeout_seconds=timeout_seconds,
            base_url=selected_base_url,
            api_key_env="GROQ_API_KEY",
            default_model=self.DEFAULT_MODEL,
            provider_name="Groq",
        )

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
        """Use Groq strict mode first, then JSON object mode on its known failure."""

        if prefer_json_object:
            return await self._respond_with_format(
                system=system,
                user=user,
                text_format={"type": "json_object"},
                max_input_chars=max_input_chars,
            )

        strict_format = {
            "type": "json_schema",
            "name": schema_name,
            "schema": schema,
            "strict": True,
        }
        try:
            return await self._respond_with_format(
                system=system,
                user=user,
                text_format=strict_format,
                max_input_chars=max_input_chars,
            )
        except LLMError as exc:
            if exc.error_code != "json_validate_failed":
                raise
        # Groq's GPT-OSS strict decoder can intermittently reject its own
        # generation. JSON object mode preserves valid JSON; Role B performs
        # the supplied schema validation immediately after this client returns.
        return await self._respond_with_format(
            system=system,
            user=user,
            text_format={"type": "json_object"},
            max_input_chars=max_input_chars,
        )

    async def _respond_with_format(
        self,
        *,
        system: str,
        user: str,
        text_format: dict,
        max_input_chars: int | None,
    ) -> dict:
        async def request() -> Any:
            client = self._get_client()
            return await client.responses.create(
                model=self.model,
                instructions=redact_for_prompt(system),
                input=redact_for_prompt(user, max_input_chars),
                text={"format": text_format},
                reasoning={"effort": "low"},
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
            raise _provider_error("LLM request or JSON parsing failed", exc) from exc
