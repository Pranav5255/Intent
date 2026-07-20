import asyncio
import json
import tempfile
from pathlib import Path

import pytest

from intent_engine.labeling import TemplateFallbackLabelProvider
from intent_engine.llm import LLMError
from intent_engine.logging import DiagnosticsLogger
from intent_engine.pipeline import run_pipeline
from intent_engine.schemas import DayExport, EventPayload, PipelineWarning, RawEvent
from intent_engine.store import IntentStore


class FakeSemanticClient:
    def __init__(self, response=None, error=None, model="gemini-test"):
        self.response = response
        self.error = error
        self.model = model
        self.calls = 0

    async def respond_json(self, **kwargs):
        self.calls += 1
        if self.error:
            raise self.error
        return self.response


@pytest.fixture(autouse=True)
def semantic_environment(monkeypatch):
    monkeypatch.delenv("ROLE_B_SEMANTIC_CLUSTER", raising=False)
    monkeypatch.delenv("ROLE_B_SEMANTIC_CONTENT_CONSENT", raising=False)
    monkeypatch.delenv("ROLE_B_LLM_ENABLED", raising=False)
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)
    monkeypatch.delenv("GEMINI_API_KEY", raising=False)
    monkeypatch.delenv("GOOGLE_APPLICATION_CREDENTIALS", raising=False)
    monkeypatch.delenv("GEMINI_CREDENTIALS_PATH", raising=False)
    monkeypatch.delenv("LLM_PROVIDER", raising=False)


def semantic_enabled(monkeypatch):
    monkeypatch.setenv("ROLE_B_SEMANTIC_CLUSTER", "true")
    monkeypatch.setenv("ROLE_B_SEMANTIC_CONTENT_CONSENT", "true")
    monkeypatch.setenv("ROLE_B_LLM_ENABLED", "true")
    monkeypatch.setenv("LLM_PROVIDER", "gemini")
    monkeypatch.setenv("GEMINI_API_KEY", "test-key")


def export():
    return DayExport(
        date="2026-07-20",
        exported_at=1,
        events=[
            RawEvent(id="terraform", ts=1, source="shell", type="command", payload=EventPayload(cmd="terraform plan", cwd="/repo/app", exit_code=0)),
            RawEvent(id="npm", ts=302, source="shell", type="command", payload=EventPayload(cmd="npm test", cwd="/repo/app", exit_code=0)),
        ],
    )


def proposals():
    return {
        "proposals": [
            {"event_id": "p0", "role": "task", "confidence": 0.9, "topic": "Infrastructure plan", "linked_event_ids": ["p1"]},
            {"event_id": "p1", "role": "supporting_context", "confidence": 0.8, "topic": "Infrastructure plan", "linked_event_ids": ["p0"]},
        ]
    }


def test_disabled_mode_matches_existing_deterministic_clusters():
    with tempfile.TemporaryDirectory() as directory:
        first_store = IntentStore(str(Path(directory) / "first.db"))
        second_store = IntentStore(str(Path(directory) / "second.db"))
        first = asyncio.run(run_pipeline(export(), first_store, TemplateFallbackLabelProvider()))
        second = asyncio.run(run_pipeline(export(), second_store, TemplateFallbackLabelProvider(), semantic_client=FakeSemanticClient(proposals())))

    assert first.model_dump(mode="json") == second.model_dump(mode="json")


def test_semantic_clusters_flow_through_enrichment_and_bounded_resume(monkeypatch):
    semantic_enabled(monkeypatch)
    client = FakeSemanticClient(proposals())
    with tempfile.TemporaryDirectory() as directory:
        result = asyncio.run(run_pipeline(export(), IntentStore(str(Path(directory) / "intents.db")), TemplateFallbackLabelProvider(), semantic_client=client))

    assert client.calls == 1
    assert len(result.intents) == 1
    intent = result.intents[0]
    assert intent.stats.event_count == 2
    assert intent.resume_payload.shell["cwd"] == "/repo/app"
    assert intent.semantic is not None
    assert intent.semantic.event_roles == {"task": 1, "supporting_context": 1}
    assert intent.semantic.topic == "Infrastructure plan"
    assert intent.semantic.provider_identity == "semantic:gemini:gemini-test:content-policy-3:cluster-policy-17"


def test_semantic_failure_falls_back_and_logs_safe_reason(monkeypatch):
    semantic_enabled(monkeypatch)
    client = FakeSemanticClient(error=LLMError("provider failed"))
    with tempfile.TemporaryDirectory() as directory:
        log_path = Path(directory) / "diagnostics.jsonl"
        result = asyncio.run(
            run_pipeline(
                export(),
                IntentStore(str(Path(directory) / "intents.db")),
                TemplateFallbackLabelProvider(),
                semantic_client=client,
                logger=DiagnosticsLogger(str(log_path)),
            )
        )
        record = json.loads(log_path.read_text(encoding="utf-8"))

    assert len(result.intents[0].children) == 2
    assert result.intents[0].semantic is None
    assert result.warnings == [PipelineWarning(level="warning", message="Semantic refinement fallback: provider_unavailable")]
    assert record["semantic_fallback_reason"] == "provider_unavailable"
    assert "provider failed" not in json.dumps(record)


def test_semantic_identity_change_misses_cache(monkeypatch):
    semantic_enabled(monkeypatch)
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        first = asyncio.run(run_pipeline(export(), store, TemplateFallbackLabelProvider(), semantic_client=FakeSemanticClient(proposals(), model="gemini-a")))
        second = asyncio.run(run_pipeline(export(), store, TemplateFallbackLabelProvider(), semantic_client=FakeSemanticClient(proposals(), model="gemini-b")))

    assert first.cached is False
    assert second.cached is False
    assert first.source_hash != second.source_hash
