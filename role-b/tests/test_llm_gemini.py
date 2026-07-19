from __future__ import annotations

import asyncio
import json
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

sys.path.insert(0, str(Path(__file__).parents[1]))

from intent_engine.llm import LLMError
from intent_engine.llm_gemini import GeminiClient


def run(coroutine):
    return asyncio.run(coroutine)


class FakeGeminiClient:
    def __init__(self) -> None:
        self.last_kwargs: dict | None = None
        self.calls = 0

    def generate_content(self, **kwargs):
        self.last_kwargs = kwargs
        self.calls += 1
        config = kwargs.get("config")
        if getattr(config, "response_mime_type", None) == "application/json":
            return SimpleNamespace(text='{"label":"Test Label","summary":"Test summary.","confidence":0.8}')
        return SimpleNamespace(
            candidates=[
                SimpleNamespace(
                    content=SimpleNamespace(
                        parts=[
                            SimpleNamespace(text="Answer text", function_call=None),
                        ]
                    )
                )
            ]
        )


def test_respond_json_uses_schema_mode(monkeypatch):
    fake = FakeGeminiClient()
    monkeypatch.setenv("GEMINI_API_KEY", "test-key")
    client = GeminiClient(api_key="test-key", model="gemini-test")
    client._client = SimpleNamespace(models=SimpleNamespace(generate_content=fake.generate_content))
    client._types = SimpleNamespace(
        GenerateContentConfig=lambda **kwargs: SimpleNamespace(**kwargs),
    )

    result = run(
        client.respond_json(
            system="system",
            user="user",
            schema_name="intent_label",
            schema={"type": "object", "properties": {"label": {"type": "string"}}, "required": ["label"]},
        )
    )

    assert result["label"] == "Test Label"
    assert fake.last_kwargs is not None
    assert fake.last_kwargs["model"] == "gemini-test"
    assert fake.last_kwargs["config"].response_mime_type == "application/json"


def test_respond_with_tools_returns_unified_shape(monkeypatch):
    fake = FakeGeminiClient()

    def generate_with_tools(**kwargs):
        fake.last_kwargs = kwargs
        return SimpleNamespace(
            candidates=[
                SimpleNamespace(
                    content=SimpleNamespace(
                        parts=[
                            SimpleNamespace(
                                text=None,
                                function_call=SimpleNamespace(name="search_intents", args={"query": "login"}),
                            )
                        ]
                    )
                )
            ]
        )

    monkeypatch.setenv("GEMINI_API_KEY", "test-key")
    client = GeminiClient(api_key="test-key")
    client._client = SimpleNamespace(models=SimpleNamespace(generate_content=generate_with_tools))
    client._types = SimpleNamespace(
        GenerateContentConfig=lambda **kwargs: SimpleNamespace(**kwargs),
        Tool=lambda function_declarations: SimpleNamespace(function_declarations=function_declarations),
        FunctionDeclaration=lambda **kwargs: SimpleNamespace(**kwargs),
        Content=lambda role, parts: SimpleNamespace(role=role, parts=parts),
        Part=SimpleNamespace(
            from_text=lambda text: SimpleNamespace(text=text),
            from_function_call=lambda name, args: SimpleNamespace(function_call=SimpleNamespace(name=name, args=args)),
            from_function_response=lambda name, response: SimpleNamespace(function_response=SimpleNamespace(name=name, response=response)),
        ),
    )

    result = run(
        client.respond_with_tools(
            system="system",
            messages=[{"role": "user", "content": "Find login work"}],
            tools=[{"type": "function", "function": {"name": "search_intents", "description": "Search", "parameters": {"type": "object"}}}],
        )
    )

    assert result["tool_calls"][0]["name"] == "search_intents"
    assert result["tool_calls"][0]["arguments"] == {"query": "login"}
    assert result["response_items"][0]["role"] == "model"


def test_respond_json_raises_on_bad_json(monkeypatch):
    fake = FakeGeminiClient()

    def bad_json(**kwargs):
        return SimpleNamespace(text="not-json")

    monkeypatch.setenv("GEMINI_API_KEY", "test-key")
    client = GeminiClient(api_key="test-key")
    client._client = SimpleNamespace(models=SimpleNamespace(generate_content=bad_json))
    client._types = SimpleNamespace(GenerateContentConfig=lambda **kwargs: SimpleNamespace(**kwargs))

    with pytest.raises(LLMError):
        run(client.respond_json(system="s", user="u", schema_name="x", schema={"type": "object"}))


def test_missing_sdk_raises_llm_error(monkeypatch):
    monkeypatch.setenv("GEMINI_API_KEY", "test-key")
    client = GeminiClient(api_key="test-key")
    client._get_client = lambda: (_ for _ in ()).throw(LLMError("Optional Gemini SDK is unavailable; install requirements-gemini.txt"))
    with pytest.raises(LLMError):
        run(client.respond_json(system="s", user="u", schema_name="x", schema={"type": "object"}))


def test_service_account_path_is_accepted(tmp_path):
    credentials = tmp_path / "sa.json"
    credentials.write_text(
        '{"type":"service_account","project_id":"kube-orch","private_key":"x"}',
        encoding="utf-8",
    )
    client = GeminiClient(credentials_path=str(credentials), project="kube-orch", location="us-central1")
    assert client.api_key is None
    assert client.credentials_path == str(credentials)
    assert client.project == "kube-orch"


def test_missing_credentials_raises():
    with pytest.raises(ValueError, match="GEMINI_API_KEY or GOOGLE_APPLICATION_CREDENTIALS"):
        GeminiClient(api_key="", credentials_path="")
