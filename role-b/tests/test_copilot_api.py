import os
import tempfile
from pathlib import Path

from fastapi.testclient import TestClient

os.environ.setdefault("ROLE_B_DB_PATH", str(Path(tempfile.gettempdir()) / "role-b-copilot-api-import.db"))

from intent_engine.api import create_app
from intent_engine.schemas import Intent, IntentInsights, IntentStats, PipelineResult, ResumePayload
from intent_engine.store import IntentStore


class FakeLLM:
    async def respond_with_tools(self, **kwargs):
        return {"output_text": "No matching stored evidence.", "tool_calls": []}


class GroundedFakeLLM:
    def __init__(self):
        self.calls = []

    async def respond_with_tools(self, **kwargs):
        self.calls.append(kwargs)
        if len(self.calls) == 1:
            return {"output_text": None, "tool_calls": [{"name": "search_intents", "arguments": {"query": "Safe"}, "call_id": "search-1"}]}
        return {"output_text": "The stored intent was Safe Work.", "tool_calls": []}


def test_copilot_disabled_returns_not_configured(monkeypatch):
    monkeypatch.delenv("ENABLE_COPILOT", raising=False)
    monkeypatch.delenv("ROLE_B_LLM_ENABLED", raising=False)
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)
    with tempfile.TemporaryDirectory() as directory:
        client = TestClient(create_app(IntentStore(str(Path(directory) / "intents.db"))))
        response = client.post("/copilot/query", json={"question": "What did I do?"})
        assert response.status_code == 503
        assert response.json()["code"] == "copilot_not_configured"


def test_copilot_enabled_fake_llm_returns_grounded_response(monkeypatch):
    monkeypatch.setenv("ENABLE_COPILOT", "true")
    monkeypatch.setenv("ROLE_B_LLM_ENABLED", "true")
    monkeypatch.setenv("OPENAI_API_KEY", "test-key")
    monkeypatch.setattr("intent_engine.api.create_copilot_llm", lambda: FakeLLM())
    monkeypatch.setattr("intent_engine.api.copilot_enabled", lambda: True)
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        intent = Intent(id="api-copilot", date="2026-07-13", label="Safe Work", summary="Stored summary", start_ts=1, end_ts=2, depth=0, stats=IntentStats(event_count=1, duration_seconds=1), insights=IntentInsights(), resume_payload=ResumePayload())
        import asyncio
        asyncio.run(store.save_pipeline_run("2026-07-13", PipelineResult(intents=[intent], source_hash="api", pipeline_version="v1")))
        response = TestClient(create_app(store)).post("/copilot/query", json={"question": "Summarize my work"})
        assert response.status_code == 200
        assert response.json()["evidence_status"] == "insufficient"
        assert "raw" not in str(response.json()).lower()


def test_copilot_enabled_multi_tool_response_is_private(monkeypatch):
    monkeypatch.setenv("ENABLE_COPILOT", "true")
    monkeypatch.setenv("ROLE_B_LLM_ENABLED", "true")
    monkeypatch.setenv("OPENAI_API_KEY", "sk-test-secret")
    fake = GroundedFakeLLM()
    monkeypatch.setattr("intent_engine.api.create_copilot_llm", lambda: fake)
    monkeypatch.setattr("intent_engine.api.copilot_enabled", lambda: True)
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        intent = Intent(id="api-grounded", date="2026-07-13", label="Safe Work", summary="Stored summary", start_ts=1, end_ts=2, depth=0, stats=IntentStats(event_count=1, duration_seconds=1), insights=IntentInsights(), resume_payload=ResumePayload())
        import asyncio
        asyncio.run(store.save_pipeline_run("2026-07-13", PipelineResult(intents=[intent], source_hash="api-grounded", pipeline_version="v1")))
        response = TestClient(create_app(store)).post("/copilot/query", json={"question": "Summarize safe work"})
        assert response.status_code == 200
        body = response.json()
        assert body["citations"][0]["intent_id"] == "api-grounded"
        assert "sk-test-secret" not in str(body)
        assert all("arguments" not in call for call in body["tool_calls_made"] if isinstance(call, dict))


def test_copilot_invalid_body_is_rejected():
    with tempfile.TemporaryDirectory() as directory:
        client = TestClient(create_app(IntentStore(str(Path(directory) / "intents.db"))))
        assert client.post("/copilot/query", json={"question": ""}).status_code == 422
