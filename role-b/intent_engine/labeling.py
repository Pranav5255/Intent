"""Cluster-label providers with template fallback and optional LLM labeling."""

from __future__ import annotations

import asyncio
import re
from abc import ABC, abstractmethod
from collections import Counter

from intent_engine.llm_base import LLMClient

_INTENT_LABEL_SCHEMA = {
    "type": "object",
    "properties": {
        "label": {"type": "string"},
        "summary": {"type": "string"},
        "confidence": {"type": "number", "minimum": 0, "maximum": 1},
    },
    "required": ["label", "summary", "confidence"],
    "additionalProperties": False,
}


class LabelProvider(ABC):
    """Interface for assigning concise labels to privacy-safe descriptions."""

    @abstractmethod
    async def label_cluster(
        self,
        cluster_events_text: str,
        project_tag: str | None = None,
        hints: dict | None = None,
    ) -> dict:
        """Return a label, one-sentence summary, and confidence."""

    async def label_parent(
        self,
        parent_events_text: str,
        project_tag: str | None = None,
        hints: dict | None = None,
    ) -> dict:
        return await self.label_cluster(parent_events_text, project_tag, hints)

    @property
    def cache_identity(self) -> str:
        """Stable non-secret identity used to separate pipeline cache variants."""

        return f"{type(self).__module__}.{type(self).__qualname__}"


class TemplateFallbackLabelProvider(LabelProvider):
    """Deterministic labels derived from cluster signals, not keyword heuristics."""

    @property
    def cache_identity(self) -> str:
        return "template-fallback-v1"

    async def label_cluster(
        self,
        cluster_events_text: str,
        project_tag: str | None = None,
        hints: dict | None = None,
    ) -> dict:
        lines = _safe_lines(cluster_events_text)
        hint_values = hints or {}
        normalized = "\n".join(lines).lower()
        command_family = _string_hint(hint_values, "command_family")
        top_file = _basename(_string_hint(hint_values, "top_file"))
        top_domain = _domain_root(_string_hint(hint_values, "top_domain"))
        dominant_family = _string_hint(hint_values, "dominant_family")

        if command_family and ("exit code" in normalized or "(failed)" in normalized):
            label = f"Debug {_title_case(command_family)} Command"
            confidence = 0.85
        elif command_family:
            label = f"Run {_title_case(command_family)}"
            confidence = 0.8
        elif top_file and dominant_family == "editor":
            label = f"Edit {_truncate(top_file, 40)}"
            confidence = 0.75
        elif top_domain and dominant_family == "browser":
            label = f"Research {_truncate(top_domain, 40)}"
            confidence = 0.75
        elif dominant_family == "focus":
            label = "Work Session"
            confidence = 0.5
        else:
            label = "Work Task"
            confidence = 0.55

        return validate_label_result(_result(label, _summary(lines), confidence))

    async def label_parent(
        self,
        parent_events_text: str,
        project_tag: str | None = None,
        hints: dict | None = None,
    ) -> dict:
        lines = _safe_lines(parent_events_text)
        hint_values = hints or {}
        project_name = _project_display_name(project_tag)
        command_families = hint_values.get("command_families") or []

        if project_name:
            label = f"Work on {_truncate(project_name, 40)}"
            confidence = 0.7
        elif len(command_families) == 2:
            label = f"{_title_case(command_families[0])} and {_title_case(command_families[1])} Work"
            confidence = 0.65
        elif len(command_families) > 2:
            label = "Multi-Task Session"
            confidence = 0.6
        else:
            label = "Work Session"
            confidence = 0.5

        return validate_label_result(_result(label, _summary(lines), confidence))


# Backward-compatible alias used by older imports and docs.
FallbackLabelProvider = TemplateFallbackLabelProvider


class LLMLabelProvider(LabelProvider):
    """Provider-agnostic LLM labeler with template fallback on failure."""

    timeout_seconds = 5.0

    def __init__(self, client: LLMClient, provider_name: str) -> None:
        self._client = client
        self._provider_name = provider_name.strip().lower() or "llm"
        self.model = client.model
        self._fallback = TemplateFallbackLabelProvider()

    @property
    def cache_identity(self) -> str:
        return f"{self._provider_name}:{self.model}"

    async def label_cluster(
        self,
        cluster_events_text: str,
        project_tag: str | None = None,
        hints: dict | None = None,
    ) -> dict:
        safe_text = "\n".join(_safe_lines(cluster_events_text))[:1200]
        try:
            response = await asyncio.wait_for(
                self._completion("cluster", safe_text, project_tag),
                timeout=self.timeout_seconds,
            )
            return validate_label_result(response)
        except Exception:
            return await self._fallback.label_cluster(safe_text, project_tag, hints)

    async def label_parent(
        self,
        parent_events_text: str,
        project_tag: str | None = None,
        hints: dict | None = None,
    ) -> dict:
        safe_text = "\n".join(_safe_lines(parent_events_text))[:1200]
        try:
            response = await asyncio.wait_for(
                self._completion("parent", safe_text, project_tag),
                timeout=self.timeout_seconds,
            )
            return validate_label_result(response)
        except Exception:
            return await self._fallback.label_parent(safe_text, project_tag, hints)

    async def _completion(self, mode: str, safe_text: str, project_tag: str | None) -> dict:
        if mode == "parent":
            user = (
                "Given these child intent labels and summaries, provide a 2-5 word parent label, "
                "a one-sentence summary, and confidence from 0 to 1.\n"
                f"Project tag: {project_tag or 'none'}\nChildren:\n{safe_text}"
            )
        else:
            user = (
                "Given these privacy-safe activity descriptions, provide a 2-5 word label, "
                "a one-sentence summary, and confidence from 0 to 1.\n"
                f"Project tag: {project_tag or 'none'}\nEvents:\n{safe_text}"
            )
        return await self._client.respond_json(
            system="Return only a JSON object with label, summary, and confidence.",
            user=user,
            schema_name="intent_label",
            schema=_INTENT_LABEL_SCHEMA,
        )


# Backward-compatible wrapper for tests that construct OpenAILabelProvider directly.
class OpenAILabelProvider(LLMLabelProvider):
    def __init__(self, api_key: str | None = None, model: str | None = None) -> None:
        from intent_engine.llm import OpenAIResponsesClient

        client = OpenAIResponsesClient(
            api_key=api_key,
            model=model,
            timeout_seconds=LLMLabelProvider.timeout_seconds,
        )
        super().__init__(client, "openai")


def validate_label_result(result: dict) -> dict:
    """Validate and normalize the common provider output contract."""
    if not isinstance(result, dict):
        raise ValueError("label result must be a dictionary")
    label, summary, confidence = result.get("label"), result.get("summary"), result.get("confidence")
    if not isinstance(label, str) or not 2 <= len(label.split()) <= 5:
        raise ValueError("label must contain 2-5 words")
    if not isinstance(summary, str) or not summary.strip():
        raise ValueError("summary must be a non-empty sentence")
    if len([part for part in re.split(r"(?<=[A-Za-z])[.!?]+(?=\s|$)", summary.strip()) if part.strip()]) != 1:
        raise ValueError("summary must contain one sentence")
    if isinstance(confidence, bool) or not isinstance(confidence, (int, float)) or not 0 <= confidence <= 1:
        raise ValueError("confidence must be between 0 and 1")
    return {"label": label.strip(), "summary": summary.strip(), "confidence": float(confidence)}


def build_cluster_hints(cluster) -> dict:
    """Build deterministic labeling hints from normalized cluster events."""
    from intent_engine.cluster import ClusterEngine

    topic = ClusterEngine()._get_topic_score(cluster)
    families = Counter(event.family for event in cluster if event.family not in {"idle"})
    dominant_family = families.most_common(1)[0][0] if families else None
    return {
        "command_family": topic.get("command_family"),
        "top_file": topic.get("top_file"),
        "top_domain": topic.get("top_domain"),
        "dominant_family": dominant_family,
    }


def build_parent_hints(command_families: list[str], project_tag: str | None) -> dict:
    """Aggregate child command families for parent labeling."""
    unique = list(dict.fromkeys(family for family in command_families if family))
    return {"command_families": unique, "project_tag": project_tag}


def _safe_lines(cluster_events_text: str) -> list[str]:
    if not isinstance(cluster_events_text, str):
        return []
    banned = re.compile(r"://|\[redacted\]|\b(?:raw|payload|document|content|url)\b", re.IGNORECASE)
    return [line.strip() for line in cluster_events_text.splitlines() if line.strip() and not banned.search(line)]


def _summary(lines: list[str]) -> str:
    description = "; ".join(lines[:3]).rstrip(".!? ")
    return f"{description}." if description else "Inferred work activity."


def _result(label: str, summary: str, confidence: float) -> dict:
    return {"label": label, "summary": summary, "confidence": confidence}


def _string_hint(hints: dict, key: str) -> str | None:
    value = hints.get(key)
    return value if isinstance(value, str) and value else None


def _basename(value: str | None) -> str | None:
    if not value:
        return None
    return value.replace("\\", "/").rsplit("/", 1)[-1] or None


def _domain_root(domain: str | None) -> str | None:
    if not domain:
        return None
    parts = domain.split(".")
    if len(parts) >= 2:
        return ".".join(parts[-2:])
    return domain


def _project_display_name(project_tag: str | None) -> str | None:
    if not project_tag:
        return None
    if project_tag.startswith("project:"):
        return project_tag.split(":", 1)[1]
    return project_tag


def _title_case(value: str) -> str:
    return value[:1].upper() + value[1:] if value else value


def _truncate(value: str, limit: int) -> str:
    return value if len(value) <= limit else value[: limit - 3] + "..."
