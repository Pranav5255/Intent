import asyncio
import json
from types import SimpleNamespace

import pytest

from intent_engine.llm import GroqResponsesClient, LLMError, OpenAIResponsesClient, _provider_error, redact_for_prompt


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


class SequencedResponses:
    def __init__(self, outcomes):
        self.outcomes = list(outcomes)
        self.calls = []

    async def create(self, **kwargs):
        self.calls.append(kwargs)
        outcome = self.outcomes.pop(0)
        if isinstance(outcome, Exception):
            raise outcome
        return outcome


def client_with(response, timeout=1.0, delay=0):
    client = OpenAIResponsesClient(api_key="test-key", timeout_seconds=timeout)
    fake = FakeResponses(response, delay)
    client._client = SimpleNamespace(responses=fake)
    return client, fake


def test_missing_key_raises(monkeypatch):
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)
    with pytest.raises(ValueError, match="OPENAI_API_KEY"):
        OpenAIResponsesClient()


def test_groq_missing_key_raises(monkeypatch):
    monkeypatch.delenv("GROQ_API_KEY", raising=False)
    with pytest.raises(ValueError, match="GROQ_API_KEY"):
        GroqResponsesClient()


def test_groq_client_uses_compatible_endpoint_and_default_model(monkeypatch):
    created = {}

    class FakeAsyncOpenAI:
        def __init__(self, **kwargs):
            created.update(kwargs)

    monkeypatch.delenv("INTENT_OS_LLM_MODEL", raising=False)
    monkeypatch.delenv("GROQ_BASE_URL", raising=False)
    monkeypatch.setattr(
        "intent_engine.llm.importlib.import_module",
        lambda name: SimpleNamespace(AsyncOpenAI=FakeAsyncOpenAI),
    )

    client = GroqResponsesClient(api_key="groq-test-key", timeout_seconds=12.5)
    assert client.model == "openai/gpt-oss-20b"
    assert client.base_url == "https://api.groq.com/openai/v1"
    assert isinstance(client._get_client(), FakeAsyncOpenAI)
    assert created == {
        "api_key": "groq-test-key",
        "base_url": "https://api.groq.com/openai/v1",
        "timeout": 12.5,
    }


def test_groq_falls_back_to_json_object_after_strict_generation_failure():
    class StrictGenerationFailure(Exception):
        status_code = 400
        code = "json_validate_failed"
        response = SimpleNamespace(headers={})

    client = GroqResponsesClient(api_key="groq-test-key", timeout_seconds=1)
    fake = SequencedResponses([
        StrictGenerationFailure(),
        SimpleNamespace(output_text='{"answer":"ok"}', output=[]),
    ])
    client._client = SimpleNamespace(responses=fake)

    result = asyncio.run(client.respond_json(system="JSON only", user="user", schema_name="answer", schema={"type": "object"}))

    assert result == {"answer": "ok"}
    assert fake.calls[0]["text"]["format"]["type"] == "json_schema"
    assert fake.calls[1]["text"]["format"] == {"type": "json_object"}
    assert fake.calls[1]["reasoning"] == {"effort": "low"}


def test_groq_can_prefer_json_object_without_a_strict_attempt():
    client = GroqResponsesClient(api_key="groq-test-key", timeout_seconds=1)
    fake = SequencedResponses([SimpleNamespace(output_text='{"answer":"ok"}', output=[])])
    client._client = SimpleNamespace(responses=fake)

    result = asyncio.run(client.respond_json(
        system="JSON only",
        user="user",
        schema_name="answer",
        schema={"type": "object"},
        prefer_json_object=True,
    ))

    assert result == {"answer": "ok"}
    assert len(fake.calls) == 1
    assert fake.calls[0]["text"]["format"] == {"type": "json_object"}


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


def test_redact_for_prompt_can_preserve_a_pre_bounded_full_capture_packet():
    value = "topic " + ("x" * 5_000)

    assert redact_for_prompt(value, None) == value


def test_provider_error_preserves_only_safe_retry_metadata():
    class ProviderFailure(Exception):
        status_code = 429
        code = "rate_limit_exceeded"
        response = SimpleNamespace(headers={"retry-after": "7.5s"})

    error = _provider_error("LLM request failed", ProviderFailure())

    assert str(error) == "LLM request failed"
    assert error.status_code == 429
    assert error.retry_after_seconds == 7.5
    assert error.error_code == "rate_limit_exceeded"


def test_provider_error_reads_nested_provider_code_without_copying_the_body():
    class ProviderFailure(Exception):
        status_code = 400
        code = None
        body = {"error": {"code": "json_validate_failed", "message": "not copied"}}
        response = SimpleNamespace(headers={})

    error = _provider_error("LLM request failed", ProviderFailure())

    assert error.status_code == 400
    assert error.error_code == "json_validate_failed"
    assert "not copied" not in str(error)


def test_provider_error_uses_token_reset_header_when_retry_after_is_absent():
    class ProviderFailure(Exception):
        status_code = 429
        code = "rate_limit_exceeded"
        response = SimpleNamespace(headers={"x-ratelimit-reset-tokens": "61.25s"})

    error = _provider_error("LLM request failed", ProviderFailure())

    assert error.retry_after_seconds == 61.25
