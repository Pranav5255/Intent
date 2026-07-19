"""Environment-driven factories for optional Role B providers."""

from __future__ import annotations

import os
from pathlib import Path

from intent_engine.labeling import LLMLabelProvider, LabelProvider, TemplateFallbackLabelProvider
from intent_engine.llm import OpenAIResponsesClient
from intent_engine.llm_base import LLMClient


SEMANTIC_CONTENT_POLICY_VERSION = "1"
_DEFAULT_SEMANTIC_TIMEOUT_MS = 8000


def _truthy(name: str) -> bool:
    return os.environ.get(name, "").strip().lower() in {"true", "1", "yes"}


def _provider_name() -> str:
    return os.environ.get("LLM_PROVIDER", "openai").strip().lower()


def _gemini_credentials_path() -> str:
    return (
        os.environ.get("GEMINI_CREDENTIALS_PATH", "").strip()
        or os.environ.get("GOOGLE_APPLICATION_CREDENTIALS", "").strip()
    )


def _gemini_configured() -> bool:
    if os.environ.get("GEMINI_API_KEY", "").strip():
        return True
    credentials_path = _gemini_credentials_path()
    return bool(credentials_path) and Path(credentials_path).is_file()


def _provider_configured() -> bool:
    if _provider_name() == "gemini":
        return _gemini_configured()
    return bool(os.environ.get("OPENAI_API_KEY", "").strip())


def llm_enabled() -> bool:
    return _truthy("ROLE_B_LLM_ENABLED") and _provider_configured()


def copilot_enabled() -> bool:
    return _truthy("ENABLE_COPILOT") and llm_enabled()


def semantic_timeout_ms() -> int:
    """Return the configured semantic-provider timeout with a safe default."""

    try:
        timeout = int(os.environ.get("ROLE_B_SEMANTIC_TIMEOUT_MS", _DEFAULT_SEMANTIC_TIMEOUT_MS))
    except (TypeError, ValueError):
        return _DEFAULT_SEMANTIC_TIMEOUT_MS
    return timeout if timeout > 0 else _DEFAULT_SEMANTIC_TIMEOUT_MS


def semantic_content_consent_granted() -> bool:
    """Return whether the user explicitly allowed semantic content sharing."""

    return _truthy("ROLE_B_SEMANTIC_CONTENT_CONSENT")


def semantic_clustering_enabled() -> bool:
    """Return whether the future semantic clustering stage may use the LLM."""

    return _truthy("ROLE_B_SEMANTIC_CLUSTER") and llm_enabled() and semantic_content_consent_granted()


def create_llm_client() -> LLMClient:
    provider = _provider_name()
    if provider == "gemini":
        from intent_engine.llm_gemini import GeminiClient

        return GeminiClient()
    if provider != "openai":
        raise ValueError(f"Unsupported LLM_PROVIDER: {provider}")
    return OpenAIResponsesClient()


def create_label_provider() -> LabelProvider:
    if not llm_enabled():
        return TemplateFallbackLabelProvider()
    return LLMLabelProvider(create_llm_client(), _provider_name())


def create_copilot_llm() -> LLMClient | None:
    return create_llm_client() if copilot_enabled() else None
