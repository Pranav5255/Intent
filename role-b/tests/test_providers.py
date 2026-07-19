import pytest

from intent_engine.labeling import FallbackLabelProvider, LLMLabelProvider, OpenAILabelProvider
from intent_engine.llm import OpenAIResponsesClient
from intent_engine.llm_gemini import GeminiClient
from intent_engine.providers import (
    copilot_enabled,
    create_copilot_llm,
    create_label_provider,
    create_llm_client,
    llm_enabled,
)


@pytest.fixture(autouse=True)
def clear_provider_env(monkeypatch):
    monkeypatch.delenv("ROLE_B_LLM_ENABLED", raising=False)
    monkeypatch.delenv("ENABLE_COPILOT", raising=False)
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)
    monkeypatch.delenv("GEMINI_API_KEY", raising=False)
    monkeypatch.delenv("LLM_PROVIDER", raising=False)
    monkeypatch.delenv("INTENT_OS_LLM_MODEL", raising=False)


def test_defaults_use_deterministic_provider():
    assert isinstance(create_label_provider(), FallbackLabelProvider)
    assert create_copilot_llm() is None
    assert not llm_enabled()


@pytest.mark.parametrize("flag", ["true", "TRUE", "1", "yes", "YeS"])
def test_truthy_llm_flag_selects_llm_provider(monkeypatch, flag):
    monkeypatch.setenv("ROLE_B_LLM_ENABLED", flag)
    monkeypatch.setenv("OPENAI_API_KEY", "test-key")
    monkeypatch.setenv("INTENT_OS_LLM_MODEL", "test-model")
    provider = create_label_provider()
    assert isinstance(provider, LLMLabelProvider)
    assert provider.model == "test-model"
    assert llm_enabled()


def test_openai_is_default_client(monkeypatch):
    monkeypatch.setenv("OPENAI_API_KEY", "test-key")
    client = create_llm_client()
    assert isinstance(client, OpenAIResponsesClient)


def test_gemini_provider_selection(monkeypatch):
    monkeypatch.setenv("LLM_PROVIDER", "gemini")
    monkeypatch.setenv("GEMINI_API_KEY", "test-key")
    client = create_llm_client()
    assert isinstance(client, GeminiClient)


def test_gemini_blank_key_disables_llm(monkeypatch):
    monkeypatch.setenv("ROLE_B_LLM_ENABLED", "true")
    monkeypatch.setenv("LLM_PROVIDER", "gemini")
    monkeypatch.setenv("GEMINI_API_KEY", "   ")
    assert isinstance(create_label_provider(), FallbackLabelProvider)
    assert not llm_enabled()


def test_copilot_requires_both_flag_and_llm(monkeypatch):
    monkeypatch.setenv("ENABLE_COPILOT", "true")
    assert not copilot_enabled()
    assert create_copilot_llm() is None


def test_copilot_factory_creates_client(monkeypatch):
    monkeypatch.setenv("ENABLE_COPILOT", "true")
    monkeypatch.setenv("ROLE_B_LLM_ENABLED", "true")
    monkeypatch.setenv("OPENAI_API_KEY", "test-key")
    client = create_copilot_llm()
    assert client is not None
    assert client.model == "gpt-4o-mini"


def test_blank_key_disables_llm(monkeypatch):
    monkeypatch.setenv("ROLE_B_LLM_ENABLED", "true")
    monkeypatch.setenv("OPENAI_API_KEY", "   ")
    assert isinstance(create_label_provider(), FallbackLabelProvider)
    assert not llm_enabled()


def test_openai_alias_still_supported():
    assert issubclass(OpenAILabelProvider, LLMLabelProvider)
