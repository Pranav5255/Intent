"""Gemini adapter implementing the Role B LLMClient protocol."""

from __future__ import annotations

import asyncio
import importlib
import json
import os
import uuid
from pathlib import Path
from typing import Any

from intent_engine.llm import LLMError, redact_for_prompt

_VERTEX_SCOPES = ("https://www.googleapis.com/auth/cloud-platform",)


def _resolve_credentials_path(
    credentials_path: str | None = None,
) -> str | None:
    raw = (
        credentials_path
        or os.environ.get("GEMINI_CREDENTIALS_PATH", "").strip()
        or os.environ.get("GOOGLE_APPLICATION_CREDENTIALS", "").strip()
        or ""
    )
    return raw or None


def _project_id_from_credentials(credentials_path: str) -> str | None:
    try:
        payload = json.loads(Path(credentials_path).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    project_id = payload.get("project_id")
    return project_id.strip() if isinstance(project_id, str) and project_id.strip() else None


class GeminiClient:
    def __init__(
        self,
        api_key: str | None = None,
        credentials_path: str | None = None,
        project: str | None = None,
        location: str | None = None,
        model: str | None = None,
        timeout_seconds: float = 60.0,
    ) -> None:
        self.api_key = (api_key if api_key is not None else os.environ.get("GEMINI_API_KEY", "")).strip() or None
        self.credentials_path = _resolve_credentials_path(credentials_path)
        self.project = (
            (project if project is not None else os.environ.get("GOOGLE_CLOUD_PROJECT", "")).strip() or None
        )
        self.location = (
            (location if location is not None else os.environ.get("GOOGLE_CLOUD_LOCATION", "")).strip()
            or "us-central1"
        )
        if not self.api_key and not self.credentials_path:
            raise ValueError(
                "GEMINI_API_KEY or GOOGLE_APPLICATION_CREDENTIALS / GEMINI_CREDENTIALS_PATH "
                "is required for live Gemini use"
            )
        self.model = model or os.environ.get("INTENT_OS_LLM_MODEL") or "gemini-2.5-flash"
        self.timeout_seconds = timeout_seconds
        self._client: Any | None = None
        self._types: Any | None = None

    def _get_client(self) -> tuple[Any, Any]:
        if self._client is not None and self._types is not None:
            return self._client, self._types
        try:
            genai = importlib.import_module("google.genai")
            if self.api_key:
                self._client = genai.Client(api_key=self.api_key)
            else:
                assert self.credentials_path is not None
                credentials_file = Path(self.credentials_path)
                if not credentials_file.is_file():
                    raise LLMError(f"Gemini credentials file not found: {self.credentials_path}")
                service_account = importlib.import_module("google.oauth2.service_account")
                credentials = service_account.Credentials.from_service_account_file(
                    str(credentials_file),
                    scopes=list(_VERTEX_SCOPES),
                )
                project = self.project or _project_id_from_credentials(str(credentials_file))
                if not project:
                    raise LLMError(
                        "GOOGLE_CLOUD_PROJECT is required when using a Gemini service account JSON"
                    )
                # Ensure ADC-style discovery works for any nested Google client usage.
                os.environ.setdefault("GOOGLE_APPLICATION_CREDENTIALS", str(credentials_file))
                self._client = genai.Client(
                    vertexai=True,
                    project=project,
                    location=self.location,
                    credentials=credentials,
                )
            self._types = genai.types
            return self._client, self._types
        except ImportError as exc:
            raise LLMError("Optional Gemini SDK is unavailable; install requirements-gemini.txt") from exc
        except LLMError:
            raise
        except Exception as exc:
            raise LLMError("Unable to initialize the Gemini client") from exc

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
        del schema_name, prefer_json_object  # Gemini uses schema body only.

        async def request() -> Any:
            client, types = self._get_client()
            config = types.GenerateContentConfig(
                system_instruction=redact_for_prompt(system),
                response_mime_type="application/json",
                response_schema=schema,
            )
            return await asyncio.to_thread(
                client.models.generate_content,
                model=self.model,
                contents=redact_for_prompt(user, max_input_chars),
                config=config,
            )

        try:
            response = await asyncio.wait_for(request(), timeout=self.timeout_seconds)
            text = self._response_text(response)
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

    async def respond_with_tools(
        self,
        *,
        system: str,
        messages: list[Any],
        tools: list[dict],
    ) -> dict:
        async def request() -> Any:
            client, types = self._get_client()
            declarations = self._openai_tools_to_declarations(tools, types)
            config = types.GenerateContentConfig(
                system_instruction=redact_for_prompt(system),
                tools=[types.Tool(function_declarations=declarations)] if declarations else None,
            )
            contents = self._messages_to_contents(messages, types)
            return await asyncio.to_thread(
                client.models.generate_content,
                model=self.model,
                contents=contents,
                config=config,
            )

        try:
            response = await asyncio.wait_for(request(), timeout=self.timeout_seconds)
            return self._translate_response(response)
        except LLMError:
            raise
        except asyncio.TimeoutError as exc:
            raise LLMError("LLM request timed out") from exc
        except Exception as exc:
            raise LLMError("LLM tool request failed") from exc

    def _translate_response(self, response: Any) -> dict:
        output_text: str | None = None
        tool_calls: list[dict] = []
        parts: list[dict] = []

        for candidate in getattr(response, "candidates", []) or []:
            content = getattr(candidate, "content", None)
            if content is None:
                continue
            for part in getattr(content, "parts", []) or []:
                text = getattr(part, "text", None)
                if isinstance(text, str) and text.strip():
                    output_text = text
                    parts.append({"text": text})
                    continue
                function_call = getattr(part, "function_call", None)
                if function_call is None:
                    continue
                name = getattr(function_call, "name", "") or ""
                args = getattr(function_call, "args", None) or {}
                if hasattr(args, "items"):
                    arguments = dict(args)
                elif isinstance(args, dict):
                    arguments = args
                else:
                    arguments = {}
                call_id = f"gemini-{uuid.uuid4().hex[:12]}"
                tool_calls.append({"name": name, "arguments": arguments, "call_id": call_id})
                parts.append(
                    {
                        "function_call": {
                            "name": name,
                            "args": arguments,
                            "call_id": call_id,
                        }
                    }
                )

        response_items = [{"role": "model", "parts": parts}] if parts else []
        return {"output_text": output_text, "tool_calls": tool_calls, "response_items": response_items}

    def _messages_to_contents(self, messages: list[Any], types: Any) -> list[Any]:
        contents: list[Any] = []
        pending_function_responses: list[Any] = []

        def flush_function_responses() -> None:
            nonlocal pending_function_responses
            if pending_function_responses:
                contents.append(types.Content(role="user", parts=pending_function_responses))
                pending_function_responses = []

        for message in messages:
            if not isinstance(message, dict):
                continue
            if message.get("type") == "function_call_output":
                name = self._function_name_for_call(message, messages)
                try:
                    payload = json.loads(message.get("output", "{}"))
                except json.JSONDecodeError:
                    payload = {"error": "invalid tool output"}
                pending_function_responses.append(
                    types.Part.from_function_response(name=name, response={"result": payload})
                )
                continue

            if message.get("role") == "model" and message.get("parts"):
                flush_function_responses()
                parts = []
                for part in message["parts"]:
                    if isinstance(part, dict) and part.get("text"):
                        parts.append(types.Part.from_text(text=str(part["text"])))
                    elif isinstance(part, dict) and part.get("function_call"):
                        call = part["function_call"]
                        parts.append(
                            types.Part.from_function_call(
                                name=str(call.get("name", "")),
                                args=call.get("args") or {},
                            )
                        )
                if parts:
                    contents.append(types.Content(role="model", parts=parts))
                continue

            role = message.get("role", "user")
            content = message.get("content")
            if isinstance(content, str) and content.strip():
                flush_function_responses()
                gemini_role = "model" if role == "assistant" else "user"
                contents.append(
                    types.Content(role=gemini_role, parts=[types.Part.from_text(text=redact_for_prompt(content))])
                )

        flush_function_responses()
        return contents

    @staticmethod
    def _function_name_for_call(output_message: dict, messages: list[Any]) -> str:
        call_id = output_message.get("call_id")
        for message in reversed(messages):
            if not isinstance(message, dict) or message.get("role") != "model":
                continue
            for part in message.get("parts") or []:
                if not isinstance(part, dict):
                    continue
                call = part.get("function_call") or {}
                if call.get("call_id") == call_id and call.get("name"):
                    return str(call["name"])
        return "tool"

    @staticmethod
    def _openai_tools_to_declarations(tools: list[dict], types: Any) -> list[Any]:
        declarations: list[Any] = []
        for tool in tools:
            if not isinstance(tool, dict):
                continue
            function = tool.get("function") if tool.get("type") == "function" else tool
            if not isinstance(function, dict):
                continue
            name = function.get("name")
            if not isinstance(name, str) or not name:
                continue
            declarations.append(
                types.FunctionDeclaration(
                    name=name,
                    description=function.get("description"),
                    parameters=function.get("parameters"),
                )
            )
        return declarations

    @staticmethod
    def _response_text(response: Any) -> str:
        text = getattr(response, "text", None)
        if isinstance(text, str) and text.strip():
            return text
        for candidate in getattr(response, "candidates", []) or []:
            content = getattr(candidate, "content", None)
            if content is None:
                continue
            for part in getattr(content, "parts", []) or []:
                part_text = getattr(part, "text", None)
                if isinstance(part_text, str) and part_text.strip():
                    return part_text
        raise ValueError("Gemini response did not contain output text")
