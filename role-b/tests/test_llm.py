import asyncio
import json
from types import SimpleNamespace

import pytest

from intent_engine.llm import LLMError, OpenAIResponsesClient, redact_for_prompt


class FakeResponses:
    def __init__(self, response=None, delay=0):
        self.response = response
        self.delay = delay
        self.calls = []

    async def create(self, **kwargs):
        self.calls.append(kwargs)
        if self.delay:
            await asyncio.sleep(self.delay)
        return self.response


def client_with(response, timeout=1.0, delay=0):
    client = OpenAIResponsesClient(api_key="test-key", timeout_seconds=timeout)
    fake = FakeResponses(response, delay)
    client._client = SimpleNamespace(responses=fake)
    return client, fake


def test_missing_key_raises(monkeypatch):
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)
    with pytest.raises(ValueError, match="OPENAI_API_KEY"):
        OpenAIResponsesClient()


def test_structured_json_response():
    client, fake = client_with(SimpleNamespace(output_text='{"answer":"ok"}', output=[]))
    result = asyncio.run(client.respond_json(system="system", user="user", schema_name="answer", schema={"type": "object"}))
    assert result == {"answer": "ok"}
    assert fake.calls[0]["text"]["format"]["type"] == "json_schema"


def test_timeout_raises_llm_error():
    client, _ = client_with(SimpleNamespace(output_text='{}', output=[]), timeout=0.01, delay=0.1)
    with pytest.raises(LLMError, match="timed out"):
        asyncio.run(client.respond_json(system="s", user="u", schema_name="x", schema={}))


def test_malformed_json_raises_llm_error():
    client, _ = client_with(SimpleNamespace(output_text="not-json", output=[]))
    with pytest.raises(LLMError):
        asyncio.run(client.respond_json(system="s", user="u", schema_name="x", schema={}))


def test_tool_response_is_normalized():
    response = SimpleNamespace(
        output_text="done",
        output=[SimpleNamespace(type="function_call", name="search_intents", arguments=json.dumps({"q": "IAM"}), call_id="call-1")],
    )
    client, _ = client_with(response)
    result = asyncio.run(client.respond_with_tools(system="s", messages=[{"role": "user", "content": "q"}], tools=[]))
    assert result["output_text"] == "done"
    assert result["tool_calls"] == [{"name": "search_intents", "arguments": {"q": "IAM"}, "call_id": "call-1"}]
    assert result["response_items"] == response.output


def test_tool_continuation_preserves_response_items_and_function_outputs():
    response = SimpleNamespace(output_text=None, output=[])
    client, fake = client_with(response)
    prior_item = SimpleNamespace(type="function_call", name="search_intents", arguments="{}", call_id="call-1")
    messages = [prior_item, {"type": "function_call_output", "call_id": "call-1", "output": "{}"}]
    asyncio.run(client.respond_with_tools(system="s", messages=messages, tools=[]))
    assert fake.calls[0]["input"] == messages


def test_redact_for_prompt_removes_secrets_and_truncates():
    text = "sk-abc123 Bearer abc.def password=hunter2 " + ("x" * 5000)
    result = redact_for_prompt(text)
    assert "sk-abc123" not in result
    assert "Bearer abc.def" not in result
    assert "password=hunter2" not in result
    assert len(result) == 4000
