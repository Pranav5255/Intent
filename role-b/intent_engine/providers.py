"""Environment-driven factories for optional Role B providers."""

from __future__ import annotations

import os

from intent_engine.labeling import LLMLabelProvider, LabelProvider, TemplateFallbackLabelProvider
from intent_engine.llm import OpenAIResponsesClient
from intent_engine.llm_base import LLMClient


def _truthy(name: str) -> bool:
    return os.environ.get(name, "").strip().lower() in {"true", "1", "yes"}


def _provider_name() -> str:
    return os.environ.get("LLM_PROVIDER", "openai").strip().lower()


def _provider_key() -> str:
    if _provider_name() == "gemini":
        return os.environ.get("GEMINI_API_KEY", "").strip()
    return os.environ.get("OPENAI_API_KEY", "").strip()


def llm_enabled() -> bool:
    return _truthy("ROLE_B_LLM_ENABLED") and bool(_provider_key())


def copilot_enabled() -> bool:
    return _truthy("ENABLE_COPILOT") and llm_enabled()


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
