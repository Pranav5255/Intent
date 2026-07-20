from __future__ import annotations

import asyncio
import json

import pytest

from intent_engine.llm import LLMError
from intent_engine.llm_bedrock import BedrockConverseClient


def run(coroutine):
    return asyncio.run(coroutine)


@pytest.fixture(autouse=True)
def clear_structured_output_env(monkeypatch):
    monkeypatch.delenv("BEDROCK_STRUCTURED_OUTPUT", raising=False)


class FakeBedrockClient:
    def __init__(self, responses: list[dict]) -> None:
        self.responses = list(responses)
        self.calls: list[dict] = []

    def converse(self, **kwargs):
        self.calls.append(kwargs)
        return self.responses.pop(0)


def response_with(*content: dict) -> dict:
    return {"output": {"message": {"role": "assistant", "content": list(content)}}}


def client_with(responses: list[dict]) -> tuple[BedrockConverseClient, FakeBedrockClient]:
    client = BedrockConverseClient(region="me-central-1", model="test-model", timeout_seconds=1)
    fake = FakeBedrockClient(responses)
    client._client = fake
    return client, fake


def test_respond_json_uses_converse_structured_output_and_sanitizes_schema():
    client, fake = client_with([response_with({"text": '{"label":"Test Label","confidence":0.8}'})])

    result = run(
        client.respond_json(
            system="system",
            user="user",
            schema_name="intent label",
            schema={
                "type": "object",
                "properties": {"confidence": {"type": "number", "minimum": 0, "maximum": 1}},
                "required": ["confidence"],
                "additionalProperties": False,
            },
        )
    )

    assert result == {"label": "Test Label", "confidence": 0.8}
    request = fake.calls[0]
    assert request["modelId"] == "test-model"
    assert request["messages"] == [{"role": "user", "content": [{"text": "user"}]}]
    assert request["outputConfig"]["textFormat"]["type"] == "json_schema"
    assert request["outputConfig"]["textFormat"]["structure"]["jsonSchema"]["name"] == "intent_label"
    schema = json.loads(request["outputConfig"]["textFormat"]["structure"]["jsonSchema"]["schema"])
    assert "minimum" not in schema["properties"]["confidence"]
    assert "maximum" not in schema["properties"]["confidence"]


def test_respond_json_honors_input_limit_and_json_prompt_mode():
    client, fake = client_with([response_with({"text": '{"ok":true}'})])

    result = run(
        client.respond_json(
            system="system",
            user="abcdefgh",
            schema_name="answer",
            schema={"type": "object"},
            max_input_chars=3,
            prefer_json_object=True,
        )
    )

    assert result == {"ok": True}
    assert "outputConfig" not in fake.calls[0]
    assert fake.calls[0]["messages"] == [{"role": "user", "content": [{"text": "abc"}]}]


def test_respond_json_accepts_a_fenced_json_prompt_response():
    client = BedrockConverseClient(region="me-central-1", model="amazon.nova-pro-v1:0", timeout_seconds=1)
    fake = FakeBedrockClient([response_with({"text": '```json\n{"ok":true}\n```'})])
    client._client = fake

    result = run(client.respond_json(system="system", user="user", schema_name="answer", schema={"type": "object"}))

    assert result == {"ok": True}


def test_tool_turn_translates_bedrock_tool_use_and_tool_results():
    tool_response = response_with(
        {"toolUse": {"toolUseId": "tool-1", "name": "search_intents", "input": {"query": "IAM"}}}
    )
    final_response = response_with({"text": "Found one stored intent."})
    client, fake = client_with([tool_response, final_response])
    tools = [
        {
            "type": "function",
            "name": "search_intents",
            "description": "Search",
            "parameters": {
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"],
                "additionalProperties": False,
            },
        }
    ]
    initial_messages = [{"role": "user", "content": "Find IAM work"}]

    first = run(client.respond_with_tools(system="system", messages=initial_messages, tools=tools))
    assert first["tool_calls"] == [{"name": "search_intents", "arguments": {"query": "IAM"}, "call_id": "tool-1"}]
    assert fake.calls[0]["toolConfig"]["tools"][0]["toolSpec"]["name"] == "search_intents"

    continuation = [
        *initial_messages,
        *first["response_items"],
        {"type": "function_call_output", "call_id": "tool-1", "output": '{"results":[{"id":"intent-1"}]}'},
    ]
    second = run(client.respond_with_tools(system="system", messages=continuation, tools=tools))

    assert second["output_text"] == "Found one stored intent."
    assert fake.calls[1]["messages"] == [
        {"role": "user", "content": [{"text": "Find IAM work"}]},
        {"role": "assistant", "content": tool_response["output"]["message"]["content"]},
        {
            "role": "user",
            "content": [
                {"toolResult": {"toolUseId": "tool-1", "content": [{"json": {"results": [{"id": "intent-1"}]}}]}}
            ],
        },
    ]


def test_adjacent_user_messages_are_combined_for_converse():
    client, fake = client_with([response_with({"text": "done"})])

    run(
        client.respond_with_tools(
            system="system",
            messages=[{"role": "user", "content": "context"}, {"role": "user", "content": "question"}],
            tools=[],
        )
    )

    assert fake.calls[0]["messages"] == [{"role": "user", "content": [{"text": "context"}, {"text": "question"}]}]


def test_tool_without_description_gets_a_valid_bedrock_description():
    client, _ = client_with([])
    tools = client._tools_to_bedrock(
        [{"type": "function", "name": "lookup", "parameters": {"type": "object", "properties": {}, "additionalProperties": False}}]
    )
    assert tools[0]["toolSpec"]["description"] == "Use lookup."


def test_json_falls_back_when_model_rejects_structured_output():
    class FallbackClient(BedrockConverseClient):
        def __init__(self):
            super().__init__(region="me-central-1", model="test-model")
            self.requests: list[dict] = []

        async def _converse(self, request):
            self.requests.append(request)
            if "outputConfig" in request:
                raise LLMError("Bedrock ValidationException: output configuration is not supported")
            return response_with({"text": '{"ok":true}'})

    client = FallbackClient()
    result = run(client.respond_json(system="system", user="user", schema_name="answer", schema={"type": "object"}))

    assert result == {"ok": True}
    assert "outputConfig" in client.requests[0]
    assert "outputConfig" not in client.requests[1]


def test_nova_pro_skips_unsupported_structured_output():
    client = BedrockConverseClient(region="me-central-1", model="amazon.nova-pro-v1:0", timeout_seconds=1)
    fake = FakeBedrockClient([response_with({"text": '{"ok":true}'})])
    client._client = fake

    result = run(client.respond_json(system="system", user="user", schema_name="answer", schema={"type": "object"}))

    assert client.structured_output is False
    assert result == {"ok": True}
    assert "outputConfig" not in fake.calls[0]


def test_timeout_raises_llm_error():
    class SlowClient:
        def converse(self, **kwargs):
            import time

            time.sleep(0.1)
            return response_with({"text": "{}"})

    client = BedrockConverseClient(region="me-central-1", model="test", timeout_seconds=0.01)
    client._client = SlowClient()
    with pytest.raises(LLMError, match="timed out"):
        run(client.respond_json(system="s", user="u", schema_name="x", schema={"type": "object"}))


def test_missing_sdk_raises_llm_error():
    client = BedrockConverseClient(region="me-central-1")
    client._get_client = lambda: (_ for _ in ()).throw(
        LLMError("Optional Bedrock SDK is unavailable; install requirements-bedrock.txt")
    )
    with pytest.raises(LLMError, match="requirements-bedrock"):
        run(client.respond_json(system="s", user="u", schema_name="x", schema={"type": "object"}))
