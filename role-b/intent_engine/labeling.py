"""Phase 3-ready cluster-label providers with a deterministic safe fallback."""

from __future__ import annotations

import asyncio
import os
import re
from abc import ABC, abstractmethod

from intent_engine.llm import OpenAIResponsesClient


class LabelProvider(ABC):
    """Interface for assigning concise labels to privacy-safe descriptions."""

    @abstractmethod
    async def label_cluster(self, cluster_events_text: str, project_tag: str | None = None) -> dict:
        """Return a label, one-sentence summary, and confidence."""

    async def label_parent(self, parent_events_text: str, project_tag: str | None = None) -> dict:
        return await self.label_cluster(parent_events_text, project_tag)

    @property
    def cache_identity(self) -> str:
        """Stable non-secret identity used to separate pipeline cache variants."""

        return f"{type(self).__module__}.{type(self).__qualname__}"


class FallbackLabelProvider(LabelProvider):
    """Deterministic labels that require no API key or network access."""

    @property
    def cache_identity(self) -> str:
        return "fallback-v1"

    async def label_cluster(self, cluster_events_text: str, project_tag: str | None = None) -> dict:
        lines = _safe_lines(cluster_events_text)
        normalized = "\n".join(lines).lower()
        if "terraform apply" in normalized:
            result = _result("Run Terraform Apply", _summary(lines), 0.9)
        elif "iam" in normalized and ("edit" in normalized or "edited" in normalized):
            result = _result("Edit IAM Trust Policy", _summary(lines), 0.85)
        elif normalized.count("docs") + normalized.count("documentation") >= 3:
            result = _result("Research Documentation", _summary(lines), 0.8)
        elif "git" in normalized or "push" in normalized:
            result = _result("Run Git Push", _summary(lines), 0.85)
        elif _shell_command_count(lines) >= 2:
            result = _result("Execute Commands", _summary(lines), 0.7)
        else:
            result = _result("Work Session", _summary(lines), 0.5)
        return validate_label_result(result)

    async def label_parent(self, parent_events_text: str, project_tag: str | None = None) -> dict:
        lines = _safe_lines(parent_events_text)
        normalized = "\n".join(lines).lower()
        if project_tag:
            result = _result(f"Work in {project_tag}", _summary(lines), 0.7)
        elif normalized.count("edit") + normalized.count("edited") >= 2:
            result = _result("Implementing Features", _summary(lines), 0.7)
        elif normalized.count("research") + normalized.count("docs") + normalized.count("documentation") >= 2:
            result = _result("Investigation Session", _summary(lines), 0.7)
        else:
            result = _result("Work Session", _summary(lines), 0.5)
        return validate_label_result(result)


class OpenAILabelProvider(LabelProvider):
    """Optional OpenAI provider that safely falls back on remote failures."""

    timeout_seconds = 5.0

    def __init__(self, api_key: str | None = None, model: str | None = None) -> None:
        self.api_key = api_key or os.environ.get("OPENAI_API_KEY")
        if not self.api_key:
            raise ValueError("OPENAI_API_KEY is required for OpenAILabelProvider")
        self.model = model or os.environ.get("INTENT_OS_LLM_MODEL", "gpt-4o-mini")
        self._client: OpenAIResponsesClient | None = None
        self._fallback = FallbackLabelProvider()

    @property
    def cache_identity(self) -> str:
        return f"openai:{self.model}"

    async def label_cluster(self, cluster_events_text: str, project_tag: str | None = None) -> dict:
        safe_text = "\n".join(_safe_lines(cluster_events_text))[:1200]
        try:
            response = await asyncio.wait_for(self._completion(safe_text, project_tag), timeout=self.timeout_seconds)
            return validate_label_result(response)
        except Exception:
            return await self._fallback.label_cluster(safe_text, project_tag)

    async def _completion(self, safe_text: str, project_tag: str | None) -> dict:
        client = self._client or OpenAIResponsesClient(api_key=self.api_key, model=self.model, timeout_seconds=self.timeout_seconds)
        self._client = client
        return await client.respond_json(
            system="Return only a JSON object with label, summary, and confidence.",
            user=(
                "Given these privacy-safe activity descriptions, provide a 2-5 word label, "
                "a one-sentence summary, and confidence from 0 to 1.\n"
                f"Project tag: {project_tag or 'none'}\nEvents:\n{safe_text}"
            ),
            schema_name="intent_label",
            schema={
                "type": "object",
                "properties": {
                    "label": {"type": "string"},
                    "summary": {"type": "string"},
                    "confidence": {"type": "number", "minimum": 0, "maximum": 1},
                },
                "required": ["label", "summary", "confidence"],
                "additionalProperties": False,
            },
        )


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


def _safe_lines(cluster_events_text: str) -> list[str]:
    if not isinstance(cluster_events_text, str):
        return []
    banned = re.compile(r"://|\[redacted\]|\b(?:raw|payload|document|content|url)\b", re.IGNORECASE)
    return [line.strip() for line in cluster_events_text.splitlines() if line.strip() and not banned.search(line)]


def _summary(lines: list[str]) -> str:
    description = "; ".join(lines[:3]).rstrip(".!? ")
    return f"{description}." if description else "Inferred work activity."


def _shell_command_count(lines: list[str]) -> int:
    return sum(line.lower().startswith("ran ") for line in lines)


def _result(label: str, summary: str, confidence: float) -> dict:
    return {"label": label, "summary": summary, "confidence": confidence}
