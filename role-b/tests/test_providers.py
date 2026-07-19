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
    semantic_clustering_enabled,
    semantic_content_consent_granted,
    semantic_timeout_ms,
)


@pytest.fixture(autouse=True)
def clear_provider_env(monkeypatch):
    monkeypatch.delenv("ROLE_B_LLM_ENABLED", raising=False)
    monkeypatch.delenv("ENABLE_COPILOT", raising=False)
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)
    monkeypatch.delenv("GEMINI_API_KEY", raising=False)
    monkeypatch.delenv("GOOGLE_APPLICATION_CREDENTIALS", raising=False)
    monkeypatch.delenv("GEMINI_CREDENTIALS_PATH", raising=False)
    monkeypatch.delenv("GOOGLE_CLOUD_PROJECT", raising=False)
    monkeypatch.delenv("GOOGLE_CLOUD_LOCATION", raising=False)
    monkeypatch.delenv("LLM_PROVIDER", raising=False)
    monkeypatch.delenv("INTENT_OS_LLM_MODEL", raising=False)
    monkeypatch.delenv("ROLE_B_SEMANTIC_CLUSTER", raising=False)
    monkeypatch.delenv("ROLE_B_SEMANTIC_TIMEOUT_MS", raising=False)
    monkeypatch.delenv("ROLE_B_SEMANTIC_CONTENT_CONSENT", raising=False)


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


def test_gemini_service_account_enables_llm(monkeypatch, tmp_path):
    credentials = tmp_path / "service-account.json"
    credentials.write_text('{"type":"service_account","project_id":"demo"}', encoding="utf-8")
    monkeypatch.setenv("ROLE_B_LLM_ENABLED", "true")
    monkeypatch.setenv("LLM_PROVIDER", "gemini")
    monkeypatch.setenv("GOOGLE_APPLICATION_CREDENTIALS", str(credentials))
    assert llm_enabled()
    client = create_llm_client()
    assert isinstance(client, GeminiClient)
    assert client.credentials_path == str(credentials)


def test_gemini_blank_key_disables_llm(monkeypatch):
    monkeypatch.setenv("ROLE_B_LLM_ENABLED", "true")
    monkeypatch.setenv("LLM_PROVIDER", "gemini")
    monkeypatch.setenv("GEMINI_API_KEY", "   ")
    assert isinstance(create_label_provider(), FallbackLabelProvider)
    assert not llm_enabled()


def test_gemini_missing_credentials_file_disables_llm(monkeypatch, tmp_path):
    monkeypatch.setenv("ROLE_B_LLM_ENABLED", "true")
    monkeypatch.setenv("LLM_PROVIDER", "gemini")
    monkeypatch.setenv("GOOGLE_APPLICATION_CREDENTIALS", str(tmp_path / "missing.json"))
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


@pytest.mark.parametrize("flag", ["true", "TRUE", "1", "yes", "YeS"])
def test_semantic_consent_uses_truthy_values(monkeypatch, flag):
    monkeypatch.setenv("ROLE_B_SEMANTIC_CONTENT_CONSENT", flag)
    assert semantic_content_consent_granted()


@pytest.mark.parametrize("flag", ["", "false", "0", "no"])
def test_semantic_consent_rejects_falsey_values(monkeypatch, flag):
    monkeypatch.setenv("ROLE_B_SEMANTIC_CONTENT_CONSENT", flag)
    assert not semantic_content_consent_granted()


def test_semantic_clustering_requires_flag_llm_key_and_consent(monkeypatch):
    monkeypatch.setenv("ROLE_B_SEMANTIC_CLUSTER", "true")
    monkeypatch.setenv("ROLE_B_LLM_ENABLED", "true")
    monkeypatch.setenv("OPENAI_API_KEY", "test-key")
    assert not semantic_clustering_enabled()

    monkeypatch.setenv("ROLE_B_SEMANTIC_CONTENT_CONSENT", "true")
    assert semantic_clustering_enabled()

    monkeypatch.setenv("ROLE_B_SEMANTIC_CLUSTER", "false")
    assert not semantic_clustering_enabled()


def test_semantic_clustering_supports_gemini(monkeypatch):
    monkeypatch.setenv("ROLE_B_SEMANTIC_CLUSTER", "true")
    monkeypatch.setenv("ROLE_B_SEMANTIC_CONTENT_CONSENT", "true")
    monkeypatch.setenv("ROLE_B_LLM_ENABLED", "true")
    monkeypatch.setenv("LLM_PROVIDER", "gemini")
    monkeypatch.setenv("GEMINI_API_KEY", "test-key")
    assert semantic_clustering_enabled()


def test_semantic_clustering_supports_gemini_service_account(monkeypatch, tmp_path):
    credentials = tmp_path / "sa.json"
    credentials.write_text('{"type":"service_account","project_id":"demo"}', encoding="utf-8")
    monkeypatch.setenv("ROLE_B_SEMANTIC_CLUSTER", "true")
    monkeypatch.setenv("ROLE_B_SEMANTIC_CONTENT_CONSENT", "true")
    monkeypatch.setenv("ROLE_B_LLM_ENABLED", "true")
    monkeypatch.setenv("LLM_PROVIDER", "gemini")
    monkeypatch.setenv("GEMINI_CREDENTIALS_PATH", str(credentials))
    assert semantic_clustering_enabled()


@pytest.mark.parametrize(
    ("value", "expected"),
    [(None, 8000), ("7500", 7500), ("invalid", 8000), ("0", 8000), ("-1", 8000)],
)
def test_semantic_timeout_uses_positive_integer_or_default(monkeypatch, value, expected):
    if value is not None:
        monkeypatch.setenv("ROLE_B_SEMANTIC_TIMEOUT_MS", value)
    assert semantic_timeout_ms() == expected
