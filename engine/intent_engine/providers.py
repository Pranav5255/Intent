"""Environment-driven factories for optional Role B providers."""

from __future__ import annotations

import os
from pathlib import Path

from intent_engine.labeling import LLMLabelProvider, LabelProvider, TemplateFallbackLabelProvider
from intent_engine.llm import GroqResponsesClient, OpenAIResponsesClient
from intent_engine.llm_base import LLMClient


SEMANTIC_CONTENT_POLICY_VERSION = "3"
# A full-capture request can carry an entire activity replay. A minute permits
# provider-side tokenization and structured output without silently falling
# back to deterministic clustering.
_DEFAULT_SEMANTIC_TIMEOUT_MS = 60_000


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
    credentials_path = _gemini_credentials_path()
    return bool(credentials_path) and Path(credentials_path).is_file()


def _groq_configured() -> bool:
    return bool(os.environ.get("GROQ_API_KEY", "").strip())


def _bedrock_configured() -> bool:
    """Return whether a Bedrock region has been selected.

    boto3 resolves credentials at request time through its standard chain. That
    allows a Bedrock API key, an AWS profile, temporary IAM credentials, or an
    attached role without requiring a particular secret in Role B's config.
    """

    return bool(
        (
            os.environ.get("BEDROCK_REGION")
            or os.environ.get("AWS_REGION")
            or os.environ.get("AWS_DEFAULT_REGION")
            or ""
        ).strip()
    )


def _provider_configured() -> bool:
    provider = _provider_name()
    if provider == "gemini":
        return _gemini_configured()
    if provider == "groq":
        return _groq_configured()
    if provider == "bedrock":
        return _bedrock_configured()
    return bool(os.environ.get("OPENAI_API_KEY", "").strip())


def llm_enabled() -> bool:
    return _truthy("ENGINE_LLM_ENABLED") and _provider_configured()


def copilot_enabled() -> bool:
    return _truthy("ENABLE_COPILOT") and llm_enabled()


def semantic_timeout_ms() -> int:
    """Return the configured semantic-provider timeout with a safe default."""

    try:
        timeout = int(os.environ.get("ENGINE_SEMANTIC_TIMEOUT_MS", _DEFAULT_SEMANTIC_TIMEOUT_MS))
    except (TypeError, ValueError):
        return _DEFAULT_SEMANTIC_TIMEOUT_MS
    return timeout if timeout > 0 else _DEFAULT_SEMANTIC_TIMEOUT_MS


def semantic_content_consent_granted() -> bool:
    """Return whether the user explicitly allowed semantic content sharing."""

    return _truthy("ENGINE_SEMANTIC_CONTENT_CONSENT")


def semantic_full_capture_consent_granted() -> bool:
    """Return whether the user explicitly opted into full captured data for semantic LLM calls."""

    return semantic_content_consent_granted() and _truthy("ENGINE_SEMANTIC_FULL_CAPTURE_CONSENT")


def semantic_clustering_enabled() -> bool:
    """Return whether the future semantic clustering stage may use the LLM."""

    return _truthy("ENGINE_SEMANTIC_CLUSTER") and llm_enabled() and semantic_content_consent_granted()


def create_llm_client(*, timeout_seconds: float | None = None) -> LLMClient:
    """Create the configured provider client with an optional request timeout."""

    provider = _provider_name()
    if provider == "gemini":
        from intent_engine.llm_gemini import GeminiClient

        return GeminiClient(timeout_seconds=timeout_seconds or 60.0)
    if provider == "groq":
        return GroqResponsesClient(timeout_seconds=timeout_seconds or 60.0)
    if provider == "bedrock":
        from intent_engine.llm_bedrock import BedrockConverseClient

        return BedrockConverseClient(timeout_seconds=timeout_seconds)
    if provider != "openai":
        raise ValueError(f"Unsupported LLM_PROVIDER: {provider}")
    return OpenAIResponsesClient(timeout_seconds=timeout_seconds or 60.0)


def create_semantic_llm_client() -> LLMClient:
    """Create a provider client whose request limit matches semantic refinement."""

    return create_llm_client(timeout_seconds=semantic_timeout_ms() / 1000)


def create_label_provider() -> LabelProvider:
    if not llm_enabled():
        return TemplateFallbackLabelProvider()
    return LLMLabelProvider(create_llm_client(), _provider_name())


def create_copilot_llm() -> LLMClient | None:
    return create_llm_client() if copilot_enabled() else None
