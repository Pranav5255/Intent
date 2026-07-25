"""Amazon Bedrock Converse API adapter for Role B."""

from __future__ import annotations

import asyncio
from concurrent.futures import ThreadPoolExecutor
from functools import partial
import importlib
import json
import os
from typing import Any

from intent_engine.llm import LLMError, redact_for_prompt


# Bedrock structured outputs accepts a useful subset of JSON Schema. Role B's
# schemas additionally contain local validation constraints that Bedrock does
# not accept in a grammar definition.
_UNSUPPORTED_STRUCTURED_SCHEMA_KEYS = {
    "exclusiveMaximum",
    "exclusiveMinimum",
    "maxItems",
    "maxLength",
    "maxProperties",
    "maximum",
    "minLength",
    "minProperties",
    "minimum",
    "multipleOf",
    "pattern",
    "uniqueItems",
}


class BedrockConverseClient:
    """Adapter over Amazon Bedrock's model-neutral Converse API.

    Authentication uses boto3's normal credential chain. It supports a
    Bedrock API key via ``AWS_BEARER_TOKEN_BEDROCK``, a named AWS profile,
    temporary IAM credentials, or an attached IAM role without copying
    credentials into Role B's application code.
    """

    def __init__(
        self,
        region: str | None = None,
        profile_name: str | None = None,
        model: str | None = None,
        timeout_seconds: float | None = None,
        max_tokens: int | None = None,
        temperature: float | None = None,
        strict_tool_use: bool | None = None,
    ) -> None:
        self.region = (
            region
            or os.environ.get("BEDROCK_REGION")
            or os.environ.get("AWS_REGION")
            or os.environ.get("AWS_DEFAULT_REGION")
        )
        self.profile_name = (
            profile_name
            or os.environ.get("BEDROCK_AWS_PROFILE")
            or os.environ.get("AWS_PROFILE")
            or None
        )
        self.model = model or os.environ.get("INTENT_LLM_MODEL") or "amazon.nova-pro-v1:0"
        self.timeout_seconds = (
            timeout_seconds
            if timeout_seconds is not None
            else _positive_float_env("BEDROCK_TIMEOUT_SECONDS", 30.0)
        )
        self.max_tokens = (
            max_tokens if max_tokens is not None else _positive_int_env("BEDROCK_MAX_TOKENS", 1024)
        )
        self.temperature = (
            temperature if temperature is not None else _temperature_env("BEDROCK_TEMPERATURE", 0.0)
        )
        self.strict_tool_use = (
            strict_tool_use if strict_tool_use is not None else _truthy("BEDROCK_STRICT_TOOL_USE")
        )
        self.structured_output = _structured_output_enabled(self.model)
        self._client: Any | None = None

    def _get_client(self) -> Any:
        if self._client is not None:
            return self._client
        try:
            boto3 = importlib.import_module("boto3")
        except ImportError as exc:
            raise LLMError("Optional Bedrock SDK is unavailable; install requirements-bedrock.txt") from exc

        try:
            session_kwargs = {"profile_name": self.profile_name} if self.profile_name else {}
            session = boto3.Session(**session_kwargs)
            region = self.region or getattr(session, "region_name", None)
            if not region:
                raise LLMError("BEDROCK_REGION or an AWS profile region is required for Bedrock")
            self._client = session.client("bedrock-runtime", region_name=region)
            return self._client
        except LLMError:
            raise
        except Exception as exc:
            raise LLMError("Unable to initialize the Amazon Bedrock client") from exc

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
        """Generate a JSON object, using Bedrock structured output when available."""

        fallback_request = self._json_prompt_request(
            system=system,
            user=user,
            schema=schema,
            max_input_chars=max_input_chars,
        )
        use_structured_output = self.structured_output and not prefer_json_object
        if use_structured_output:
            structured_request = {
                **self._base_request(
                    system=system,
                    messages=[{"role": "user", "content": user}],
                    max_input_chars=max_input_chars,
                ),
                "outputConfig": {
                    "textFormat": {
                        "type": "json_schema",
                        "structure": {
                            "jsonSchema": {
                                "name": _safe_schema_name(schema_name),
                                "description": "Role B structured response",
                                "schema": json.dumps(_bedrock_schema(schema), separators=(",", ":")),
                            }
                        },
                    }
                },
            }
            try:
                response = await self._converse(structured_request)
            except LLMError as exc:
                # Older SDK/model combinations can reject outputConfig before
                # an inference request is run. Keep an enabled comparison model
                # usable by retrying with an explicit JSON-only prompt.
                if not _structured_output_unsupported(exc):
                    raise
                response = await self._converse(fallback_request)
        else:
            response = await self._converse(fallback_request)

        try:
            parsed = _parse_json_object(self._response_text(response))
            if not isinstance(parsed, dict):
                raise ValueError("structured response must be an object")
            return parsed
        except LLMError:
            raise
        except Exception as exc:
            raise LLMError("LLM request or JSON parsing failed") from exc

    async def respond_with_tools(
        self,
        *,
        system: str,
        messages: list[Any],
        tools: list[dict],
    ) -> dict:
        """Run one Bedrock client-side tool-use turn in the shared protocol."""

        request = self._base_request(system=system, messages=messages)
        bedrock_tools = self._tools_to_bedrock(tools)
        if bedrock_tools:
            request["toolConfig"] = {"tools": bedrock_tools}

        try:
            response = await self._converse(request)
            output_message = self._output_message(response)
            return {
                "output_text": self._message_text(output_message),
                "tool_calls": self._tool_calls(output_message),
                # Bedrock requires the exact assistant message, including tool
                # IDs and reasoning signatures, in a continuation request.
                "response_items": [output_message],
            }
        except LLMError:
            raise
        except Exception as exc:
            raise LLMError("LLM tool request failed") from exc

    def _base_request(
        self,
        *,
        system: str,
        messages: list[Any],
        max_input_chars: int | None = 4_000,
    ) -> dict:
        return {
            "modelId": self.model,
            "system": [{"text": redact_for_prompt(system)}],
            "messages": self._messages_to_bedrock(messages, max_input_chars=max_input_chars),
            "inferenceConfig": {
                "maxTokens": self.max_tokens,
                "temperature": self.temperature,
            },
        }

    def _json_prompt_request(
        self,
        *,
        system: str,
        user: str,
        schema: dict,
        max_input_chars: int | None,
    ) -> dict:
        return self._base_request(
            system=(
                f"{system}\nReturn only one JSON object that conforms to this schema: "
                f"{json.dumps(_bedrock_schema(schema), separators=(',', ':'))}"
            ),
            messages=[{"role": "user", "content": user}],
            max_input_chars=max_input_chars,
        )

    async def _converse(self, request: dict) -> dict:
        # boto3 is synchronous. A short-lived executor keeps it off FastAPI's
        # event loop without retaining a default asyncio worker for the whole
        # process lifetime.
        executor = ThreadPoolExecutor(max_workers=1, thread_name_prefix="intent-bedrock")
        try:
            call = partial(self._get_client().converse, **request)
            future = asyncio.get_running_loop().run_in_executor(executor, call)
            response = await asyncio.wait_for(future, timeout=self.timeout_seconds)
            if not isinstance(response, dict):
                raise ValueError("Bedrock response was not an object")
            return response
        except LLMError:
            raise
        except asyncio.TimeoutError as exc:
            raise LLMError("LLM request timed out") from exc
        except Exception as exc:
            raise LLMError(_bedrock_error_message(exc)) from exc
        finally:
            # Do not block a timed-out request while a transport call unwinds.
            executor.shutdown(wait=False, cancel_futures=True)

    @classmethod
    def _messages_to_bedrock(
        cls,
        messages: list[Any],
        *,
        max_input_chars: int | None = 4_000,
    ) -> list[dict]:
        """Translate the shared continuation shape into Converse messages."""

        translated: list[dict] = []

        def append_message(role: str, content: list[dict]) -> None:
            if not content:
                return
            # Converse expects alternating roles. Combining adjacent blocks also
            # collects parallel tool results into one user message.
            if translated and translated[-1]["role"] == role:
                translated[-1]["content"].extend(content)
            else:
                translated.append({"role": role, "content": content})

        for message in messages:
            if not isinstance(message, dict):
                continue
            if message.get("type") == "function_call_output":
                call_id = message.get("call_id")
                if not isinstance(call_id, str) or not call_id:
                    continue
                payload = _json_output(message.get("output"))
                append_message(
                    "user",
                    [{"toolResult": {"toolUseId": call_id, "content": [{"json": _redact_value(payload)}]}}],
                )
                continue

            role = message.get("role")
            content = message.get("content")
            if role not in {"user", "assistant"}:
                continue

            # Assistant content returned by Bedrock must remain byte-for-byte
            # intact so model reasoning/tool signatures continue to validate.
            if role == "assistant" and isinstance(content, list):
                append_message("assistant", content)
                continue

            if isinstance(content, str) and content.strip():
                append_message(role, [{"text": redact_for_prompt(content, max_input_chars)}])
                continue

            if isinstance(content, list):
                text_blocks = []
                for block in content:
                    if isinstance(block, dict) and isinstance(block.get("text"), str) and block["text"].strip():
                        text_blocks.append({"text": redact_for_prompt(block["text"], max_input_chars)})
                append_message(role, text_blocks)

        return translated

    def _tools_to_bedrock(self, tools: list[dict]) -> list[dict]:
        translated: list[dict] = []
        for tool in tools:
            if not isinstance(tool, dict):
                continue
            function = (
                tool.get("function")
                if tool.get("type") == "function" and isinstance(tool.get("function"), dict)
                else tool
            )
            name = function.get("name") if isinstance(function, dict) else None
            if not isinstance(name, str) or not name:
                continue
            description = str(function.get("description") or f"Use {name}.")
            spec = {
                "name": name,
                "description": description,
                "inputSchema": {"json": _bedrock_schema(function.get("parameters") or {"type": "object"})},
            }
            if self.strict_tool_use:
                spec["strict"] = True
            translated.append({"toolSpec": spec})
        return translated

    @staticmethod
    def _output_message(response: dict) -> dict:
        output = response.get("output")
        message = output.get("message") if isinstance(output, dict) else None
        if (
            not isinstance(message, dict)
            or message.get("role") != "assistant"
            or not isinstance(message.get("content"), list)
        ):
            raise ValueError("Bedrock response did not contain an assistant message")
        return message

    @classmethod
    def _response_text(cls, response: dict) -> str:
        text = cls._message_text(cls._output_message(response))
        if not text:
            raise ValueError("Bedrock response did not contain output text")
        return text

    @staticmethod
    def _message_text(message: dict) -> str | None:
        texts = [
            block["text"]
            for block in message.get("content", [])
            if isinstance(block, dict) and isinstance(block.get("text"), str) and block["text"].strip()
        ]
        return "\n".join(texts) if texts else None

    @staticmethod
    def _tool_calls(message: dict) -> list[dict]:
        calls: list[dict] = []
        for block in message.get("content", []):
            tool_use = block.get("toolUse") if isinstance(block, dict) else None
            if not isinstance(tool_use, dict):
                continue
            name = tool_use.get("name")
            call_id = tool_use.get("toolUseId")
            arguments = tool_use.get("input")
            if not isinstance(name, str) or not isinstance(call_id, str):
                continue
            calls.append(
                {
                    "name": name,
                    "arguments": arguments if isinstance(arguments, dict) else {},
                    "call_id": call_id,
                }
            )
        return calls


def _bedrock_schema(value: Any) -> Any:
    """Return a recursively compatible JSON Schema for Bedrock grammar use."""

    if isinstance(value, dict):
        result = {}
        for key, item in value.items():
            if key in _UNSUPPORTED_STRUCTURED_SCHEMA_KEYS:
                continue
            if key == "minItems" and item not in {0, 1}:
                continue
            # Bedrock's structured-output grammar accepts only false here.
            if key == "additionalProperties" and item is not False:
                continue
            result[key] = _bedrock_schema(item)
        return result
    if isinstance(value, list):
        return [_bedrock_schema(item) for item in value]
    return value


def _redact_value(value: Any) -> Any:
    if isinstance(value, str):
        return redact_for_prompt(value)
    if isinstance(value, list):
        return [_redact_value(item) for item in value]
    if isinstance(value, dict):
        return {str(key): _redact_value(item) for key, item in value.items()}
    return value


def _json_output(value: Any) -> Any:
    if not isinstance(value, str):
        return {"result": _redact_value(value)}
    try:
        return json.loads(value)
    except json.JSONDecodeError:
        return {"error": "invalid tool output"}


def _parse_json_object(text: str) -> Any:
    """Parse a model JSON response, tolerating a Markdown code fence.

    Nova Pro normally follows the JSON-only instruction, but its prompt-based
    JSON mode may occasionally wrap the object in a ``json`` code fence. The
    result remains validated by Role B immediately after this adapter returns.
    """

    value = text.strip()
    if value.startswith("```"):
        lines = value.splitlines()
        if lines and lines[0].lstrip().startswith("```"):
            lines = lines[1:]
        if lines and lines[-1].strip().startswith("```"):
            lines = lines[:-1]
        value = "\n".join(lines).strip()
    try:
        return json.loads(value)
    except json.JSONDecodeError:
        # A bounded fallback handles a short explanatory prefix/suffix without
        # accepting arbitrary prose as a valid structured response.
        start, end = value.find("{"), value.rfind("}")
        if start < 0 or end <= start:
            raise
        return json.loads(value[start : end + 1])


def _safe_schema_name(value: str) -> str:
    cleaned = "".join(character if character.isalnum() or character in {"_", "-"} else "_" for character in value)
    return (cleaned or "structured_output")[:64]


def _structured_output_unsupported(error: LLMError) -> bool:
    message = str(error).lower()
    return (
        "outputconfig" in message
        or "output config" in message
        or "output configuration" in message
        or "structured output" in message
        or "json_schema" in message
    )


def _bedrock_error_message(error: Exception) -> str:
    response = getattr(error, "response", None)
    if isinstance(response, dict):
        details = response.get("Error")
        if isinstance(details, dict):
            code = str(details.get("Code", "BedrockError"))
            message = str(details.get("Message", "Bedrock request failed"))
            return f"Bedrock {code}: {message}"
    return "Bedrock request failed"


def _truthy(name: str) -> bool:
    return os.environ.get(name, "").strip().lower() in {"true", "1", "yes"}


def _positive_int_env(name: str, default: int) -> int:
    try:
        value = int(os.environ.get(name, str(default)))
    except ValueError:
        return default
    return value if value > 0 else default


def _positive_float_env(name: str, default: float) -> float:
    try:
        value = float(os.environ.get(name, str(default)))
    except ValueError:
        return default
    return value if value > 0 else default


def _temperature_env(name: str, default: float) -> float:
    try:
        value = float(os.environ.get(name, str(default)))
    except ValueError:
        return default
    return value if 0 <= value <= 1 else default


def _structured_output_enabled(model: str) -> bool:
    """Choose schema mode only when it is available for the selected model."""

    configured = os.environ.get("BEDROCK_STRUCTURED_OUTPUT", "auto").strip().lower()
    if configured in {"true", "1", "yes"}:
        return True
    if configured in {"false", "0", "no"}:
        return False
    # Nova Pro supports Converse and tool use but not Bedrock structured
    # output. Skip a guaranteed validation failure on every labeling request.
    return "nova-pro" not in model.lower()
