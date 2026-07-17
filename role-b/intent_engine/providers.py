"""Environment-driven factories for optional Role B providers."""

from __future__ import annotations

import os

from intent_engine.labeling import FallbackLabelProvider, LabelProvider, OpenAILabelProvider
from intent_engine.llm import OpenAIResponsesClient


def _truthy(name: str) -> bool:
    return os.environ.get(name, "").strip().lower() in {"true", "1", "yes"}


def llm_enabled() -> bool:
    return _truthy("ROLE_B_LLM_ENABLED") and bool(os.environ.get("OPENAI_API_KEY", "").strip())


def copilot_enabled() -> bool:
    return _truthy("ENABLE_COPILOT") and llm_enabled()


def create_label_provider() -> LabelProvider:
    return OpenAILabelProvider() if llm_enabled() else FallbackLabelProvider()


def create_copilot_llm() -> OpenAIResponsesClient | None:
    return OpenAIResponsesClient() if copilot_enabled() else None
