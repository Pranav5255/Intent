from __future__ import annotations

import asyncio
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

sys.path.insert(0, str(Path(__file__).parents[1]))

from intent_engine.labeling import FallbackLabelProvider, LabelProvider, OpenAILabelProvider, validate_label_result


def run(coroutine):
    return asyncio.run(coroutine)


@pytest.mark.parametrize(
    ("text", "label", "confidence"),
    [
        ("Ran terraform apply", "Run Terraform Apply", 0.9),
        ("Edited iam.tf\nEdited IAM policy", "Edit IAM Trust Policy", 0.85),
        ("Viewed docs\nRead documentation\nViewed docs", "Research Documentation", 0.8),
        ("Ran git push origin main", "Run Git Push", 0.85),
        ("Ran python test.py\nRan pytest", "Execute Commands", 0.7),
        ("Focused on editor", "Work Session", 0.5),
    ],
)
def test_fallback_cluster_heuristics(text, label, confidence) -> None:
    result = run(FallbackLabelProvider().label_cluster(text))
    assert result["label"] == label
    assert result["confidence"] == confidence
    assert result["summary"].endswith(".")


def test_parent_label_and_abstract_contract() -> None:
    with pytest.raises(TypeError):
        LabelProvider()
    provider = FallbackLabelProvider()
    assert run(provider.label_parent("Edited app\nEdited test", "project:demo"))["label"] == "Work in project:demo"
    assert run(provider.label_parent("Edited app\nEdited test"))["label"] == "Implementing Features"


def test_validation_rejects_invalid_provider_output() -> None:
    with pytest.raises(ValueError, match="2-5"):
        validate_label_result({"label": "One", "summary": "One sentence.", "confidence": 0.5})
    with pytest.raises(ValueError, match="one sentence"):
        validate_label_result({"label": "Valid Label", "summary": "First. Second.", "confidence": 0.5})
    with pytest.raises(ValueError, match="confidence"):
        validate_label_result({"label": "Valid Label", "summary": "One sentence.", "confidence": 2})


class FakeResponsesClient:
    def __init__(self, content: str) -> None:
        self.content = content
        self.calls: list[dict] = []

    async def respond_json(self, **kwargs):
        self.calls.append(kwargs)
        import json
        return json.loads(self.content)


def test_openai_provider_validates_response_and_only_sends_safe_text() -> None:
    provider = OpenAILabelProvider(api_key="test-key")
    fake_client = FakeResponsesClient('{"label":"Review IAM Policy","summary":"Reviewed IAM configuration.","confidence":0.8}')
    provider._client = fake_client
    result = run(provider.label_cluster("Edited iam.tf\nhttps://secret.example/path\nraw payload here", "project:infra"))

    assert result["label"] == "Review IAM Policy"
    prompt = fake_client.calls[0]["user"]
    assert "Edited iam.tf" in prompt
    assert "https://" not in prompt
    assert "raw payload" not in prompt
    assert fake_client.calls[0]["schema_name"] == "intent_label"


def test_openai_provider_falls_back_for_bad_json_and_missing_key(monkeypatch) -> None:
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)
    with pytest.raises(ValueError, match="OPENAI_API_KEY"):
        OpenAILabelProvider(api_key="")
    provider = OpenAILabelProvider(api_key="test-key")
    provider._client = FakeResponsesClient("not json")
    assert run(provider.label_cluster("Ran terraform apply"))["label"] == "Run Terraform Apply"


def test_openai_provider_model_resolution(monkeypatch) -> None:
    monkeypatch.setenv("INTENT_OS_LLM_MODEL", "env-model")
    assert OpenAILabelProvider(api_key="test-key").model == "env-model"
    explicit = OpenAILabelProvider(api_key="test-key", model="explicit-model")
    assert explicit.model == "explicit-model"
    assert explicit.cache_identity == "openai:explicit-model"
