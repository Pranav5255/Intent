from __future__ import annotations

import asyncio
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parents[1]))

from intent_engine.labeling import (
    FallbackLabelProvider,
    LabelProvider,
    OpenAILabelProvider,
    TemplateFallbackLabelProvider,
    validate_label_result,
)


def run(coroutine):
    return asyncio.run(coroutine)


@pytest.mark.parametrize(
    ("hints", "expected_label"),
    [
        ({"command_family": "npm", "dominant_family": "command"}, "Run Npm"),
        ({"top_file": "auth.tsx", "dominant_family": "editor"}, "Edit auth.tsx"),
        ({"top_domain": "docs.aws.amazon.com", "dominant_family": "browser"}, "Research amazon.com"),
        ({"dominant_family": "focus"}, "Work Session"),
        ({}, "Work Task"),
    ],
)
def test_template_cluster_labels(hints, expected_label) -> None:
    result = run(TemplateFallbackLabelProvider().label_cluster("1. Edited auth.tsx", hints=hints))
    assert result["label"] == expected_label
    assert result["summary"].endswith(".")


def test_template_parent_labels() -> None:
    provider = TemplateFallbackLabelProvider()
    assert run(provider.label_parent("1. child", project_tag="project:demo", hints={"command_families": []}))["label"] == "Work on demo"
    assert run(provider.label_parent("1. child", hints={"command_families": ["npm", "git"]}))["label"] == "Npm and Git Work"
    assert run(provider.label_parent("1. child", hints={"command_families": ["npm", "git", "pytest"]}))["label"] == "Multi-Task Session"


def test_parent_label_and_abstract_contract() -> None:
    with pytest.raises(TypeError):
        LabelProvider()
    provider = FallbackLabelProvider()
    assert run(provider.label_parent("Edited app\nEdited test", "project:demo", {"command_families": []}))["label"] == "Work on demo"
    assert run(provider.label_parent("Edited app\nEdited test", hints={"command_families": []}))["label"] == "Work Session"


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
        self.model = "fake-model"

    async def respond_json(self, **kwargs):
        self.calls.append(kwargs)
        import json

        return json.loads(self.content)

    async def respond_with_tools(self, **kwargs):
        raise NotImplementedError


def test_llm_provider_validates_response_and_receives_role_a_approved_context() -> None:
    provider = OpenAILabelProvider(api_key="test-key")
    fake_client = FakeResponsesClient('{"label":"Review IAM Policy","summary":"Reviewed IAM configuration.","confidence":0.8}')
    provider._client = fake_client
    result = run(provider.label_cluster("Edited iam.tf\nhttps://secret.example/path\nraw payload here", "project:infra"))

    assert result["label"] == "Review IAM Policy"
    prompt = fake_client.calls[0]["user"]
    assert "Edited iam.tf" in prompt
    assert "https://secret.example/path" in prompt
    assert "raw payload here" in prompt
    assert fake_client.calls[0]["schema_name"] == "intent_label"


def test_llm_provider_falls_back_for_bad_json_and_missing_key(monkeypatch) -> None:
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)
    with pytest.raises(ValueError, match="OPENAI_API_KEY"):
        OpenAILabelProvider(api_key="")
    provider = OpenAILabelProvider(api_key="test-key")
    provider._client = FakeResponsesClient("not json")
    assert run(provider.label_cluster("1. Ran npm", hints={"command_family": "npm", "dominant_family": "command"}))["label"] == "Run Npm"


def test_openai_provider_model_resolution(monkeypatch) -> None:
    monkeypatch.setenv("INTENT_OS_LLM_MODEL", "env-model")
    assert OpenAILabelProvider(api_key="test-key").model == "env-model"
    explicit = OpenAILabelProvider(api_key="test-key", model="explicit-model")
    assert explicit.model == "explicit-model"
    assert explicit.cache_identity == "openai:explicit-model"
