"""Cluster-label providers with template fallback and optional LLM labeling."""

from __future__ import annotations

import asyncio
import json
import re
from abc import ABC, abstractmethod
from collections import Counter

from intent_engine.llm_base import LLMClient
from intent_engine.schemas import (
    SAFE_INTENT_PRIVACY_POLICY_VERSION,
    Intent,
    NormalizedEvent,
    SafeIntentFeatures,
)

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

_SAFE_EVENT_FAMILIES = frozenset({"editor", "browser", "command", "focus", "file_change", "idle", "other"})
_SAFE_COMMAND_FAMILIES = frozenset({"terraform", "git", "pytest", "python", "npm", "pip", "docker", "make"})
_SAFE_FILE_KINDS = frozenset({"code", "pdf", "image", "other"})
SAFE_FEATURE_POLICY_VERSION = SAFE_INTENT_PRIVACY_POLICY_VERSION
_SAFE_BOUNDARY_REASONS = frozenset({
    "raw_event_content_omitted",
    "child_evidence_omitted",
    "untrusted_input_omitted",
})
_SOURCE_FAMILIES = {
    "vscode": "editor",
    "firefox": "browser",
    "chrome": "browser",
    "shell": "command",
    "linux": "focus",
    "filesystem": "file_change",
}


def build_safe_cluster_features(
    cluster: list[NormalizedEvent],
    *,
    semantic_topic: str | None = None,
    project_tag: str | None = None,
) -> SafeIntentFeatures:
    """Project a cluster into the fixed allowlist used by cloud labelers."""

    from intent_engine.cluster import ClusterEngine

    events = list(cluster)
    event_counts: Counter[str] = Counter()
    command_families: list[str] = []
    file_kinds: list[str] = []
    for event in events:
        if event.family in _SAFE_EVENT_FAMILIES:
            event_counts[event.family] += 1
        command_family = event.entities.command_family
        if command_family in _SAFE_COMMAND_FAMILIES and command_family not in command_families:
            command_families.append(command_family)
        file_kind = event.entities.file_kind
        if file_kind in _SAFE_FILE_KINDS and file_kind not in file_kinds:
            file_kinds.append(file_kind)

    topic = ClusterEngine()._get_topic_score(events)
    families = Counter(event.family for event in events if event.family not in {"idle"})
    dominant_family = families.most_common(1)[0][0] if families else None
    if dominant_family not in _SAFE_EVENT_FAMILIES:
        dominant_family = None

    domains = _safe_domain_roots(topic.get("domains") or [])
    if not domains:
        top_domain = topic.get("top_domain")
        if isinstance(top_domain, str) and top_domain:
            domains = _safe_domain_roots([top_domain])

    file_names = _safe_basenames(topic.get("file_names") or [])
    if not file_names:
        top_file = topic.get("top_file")
        if isinstance(top_file, str) and top_file:
            file_names = _safe_basenames([top_file])

    duration_seconds = max(0, events[-1].ts - events[0].ts) if events else 0
    safe_topic = _safe_semantic_topic(semantic_topic)
    project_key = _safe_project_key(project_tag)

    return SafeIntentFeatures(
        project_key=project_key,
        command_families=command_families,
        file_kinds=file_kinds,
        domains=domains,
        file_names=file_names,
        dominant_family=dominant_family,
        semantic_topic=safe_topic,
        event_counts=dict(sorted(event_counts.items())),
        duration_seconds=min(duration_seconds, 86_400),
        boundary_reasons=["raw_event_content_omitted"],
    )


def build_safe_parent_features(children: list[Intent]) -> SafeIntentFeatures:
    """Aggregate child intent metadata without reusing child labels or evidence."""

    event_counts: Counter[str] = Counter()
    command_families: list[str] = []
    domains: list[str] = []
    file_names: list[str] = []
    topics: list[str] = []
    duration_seconds = 0
    for child in children:
        duration_seconds += max(0, child.stats.duration_seconds)
        source_counted = False
        for source, count in child.stats.sources.items():
            family = _SOURCE_FAMILIES.get(source)
            if family and isinstance(count, int) and not isinstance(count, bool) and count > 0:
                event_counts[family] += count
                source_counted = True
        if not source_counted and child.stats.event_count > 0:
            event_counts["other"] += child.stats.event_count
        for insight in child.insights.shell:
            family = insight.get("command_family") if isinstance(insight, dict) else None
            if family in _SAFE_COMMAND_FAMILIES and family not in command_families:
                command_families.append(family)
        for insight in child.insights.browser:
            domain = insight.get("domain") if isinstance(insight, dict) else None
            if isinstance(domain, str):
                domains = _merge_limited(domains, _safe_domain_roots([domain]), 3)
        for insight in child.insights.editor:
            file_name = insight.get("file") if isinstance(insight, dict) else None
            if isinstance(file_name, str):
                file_names = _merge_limited(file_names, _safe_basenames([file_name]), 3)
        if child.semantic and child.semantic.topic:
            topic = _safe_semantic_topic(child.semantic.topic)
            if topic and topic not in topics:
                topics.append(topic)

    dominant_family = None
    if event_counts:
        dominant_family = event_counts.most_common(1)[0][0]
        if dominant_family not in _SAFE_EVENT_FAMILIES:
            dominant_family = None

    return SafeIntentFeatures(
        command_families=command_families,
        domains=domains,
        file_names=file_names,
        dominant_family=dominant_family,
        semantic_topic=topics[0] if len(topics) == 1 else None,
        event_counts=dict(sorted(event_counts.items())),
        duration_seconds=min(duration_seconds, 86_400),
        child_count=min(len(children), 1_000),
        boundary_reasons=["child_evidence_omitted"],
    )


def serialize_safe_features(features: SafeIntentFeatures) -> str:
    """Return a stable, provider-ready representation of safe aggregate features."""

    return json.dumps(features.model_dump(mode="json"), sort_keys=True, separators=(",", ":"))


def safe_provider_hints(hints: dict | None) -> dict:
    """Keep optional provider hints to fixed categorical values only."""

    if not isinstance(hints, dict):
        return {}
    safe: dict[str, object] = {}
    command_family = hints.get("command_family")
    if command_family in _SAFE_COMMAND_FAMILIES:
        safe["command_family"] = command_family
    dominant_family = hints.get("dominant_family")
    if dominant_family in _SAFE_EVENT_FAMILIES:
        safe["dominant_family"] = dominant_family
    command_families = _safe_string_list(hints.get("command_families"), _SAFE_COMMAND_FAMILIES, 8)
    if command_families:
        safe["command_families"] = command_families
    top_file = hints.get("top_file")
    if isinstance(top_file, str) and top_file:
        basename = _basename(top_file)
        if basename:
            safe["top_file"] = basename[:120]
    top_domain = hints.get("top_domain")
    if isinstance(top_domain, str) and top_domain:
        roots = _safe_domain_roots([top_domain])
        if roots:
            safe["top_domain"] = roots[0]
    return safe


def merge_label_provider_packet(
    features_text: str,
    hints: dict | None = None,
    project_tag: str | None = None,
) -> str:
    """Merge categorical hints into a provider-safe feature packet."""

    safe_text = _provider_safe_features(features_text)
    try:
        payload = json.loads(safe_text)
    except (TypeError, ValueError):
        return safe_text
    if not isinstance(payload, dict):
        return safe_text

    hint_values = safe_provider_hints(hints)
    if hint_values.get("dominant_family") and not payload.get("dominant_family"):
        payload["dominant_family"] = hint_values["dominant_family"]
    if hint_values.get("command_family") and hint_values["command_family"] not in payload.get("command_families", []):
        payload.setdefault("command_families", []).insert(0, hint_values["command_family"])
    if hint_values.get("top_domain"):
        payload["domains"] = _merge_limited(payload.get("domains", []), [hint_values["top_domain"]], 3)
    if hint_values.get("top_file"):
        payload["file_names"] = _merge_limited(payload.get("file_names", []), [hint_values["top_file"]], 3)
    project_key = _safe_project_key(project_tag)
    if project_key and not payload.get("project_key"):
        payload["project_key"] = project_key
    return serialize_safe_features(SafeIntentFeatures.model_validate(payload))


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
        return "template-fallback-v2"

    async def label_cluster(
        self,
        cluster_events_text: str,
        project_tag: str | None = None,
        hints: dict | None = None,
    ) -> dict:
        # The deterministic fallback deliberately uses only activity headlines.
        # It must not copy consent-approved excerpts into a public summary.
        lines = _activity_lines(cluster_events_text)
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
        lines = _activity_lines(parent_events_text)
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
        return f"{self._provider_name}:{self.model}:{SAFE_FEATURE_POLICY_VERSION}"

    async def label_cluster(
        self,
        cluster_events_text: str,
        project_tag: str | None = None,
        hints: dict | None = None,
    ) -> dict:
        # The pipeline sends a SafeIntentFeatures packet here.  Parse and
        # re-project it again so direct callers cannot bypass the cloud boundary.
        safe_text = merge_label_provider_packet(cluster_events_text, hints, project_tag)
        try:
            response = await asyncio.wait_for(
                self._completion("cluster", safe_text),
                timeout=self.timeout_seconds,
            )
            return validate_label_result(response)
        except Exception:
            # This fallback stays local, so it can retain the caller's normal
            # deterministic hints without placing them in a provider request.
            return await self._fallback.label_cluster(cluster_events_text, project_tag, hints)

    async def label_parent(
        self,
        parent_events_text: str,
        project_tag: str | None = None,
        hints: dict | None = None,
    ) -> dict:
        safe_text = merge_label_provider_packet(parent_events_text, hints, project_tag)
        try:
            response = await asyncio.wait_for(
                self._completion("parent", safe_text),
                timeout=self.timeout_seconds,
            )
            return validate_label_result(response)
        except Exception:
            return await self._fallback.label_parent(parent_events_text, project_tag, hints)

    async def _completion(self, mode: str, safe_text: str) -> dict:
        if mode == "parent":
            user = (
                "Given these safe aggregate child-intent features, provide a 2-5 word parent label, "
                "a one-sentence summary, and confidence from 0 to 1.\n"
                f"Safe features:\n{safe_text}"
            )
        else:
            user = (
                "Given these safe aggregate activity features, provide a 2-5 word label, "
                "a one-sentence summary, and confidence from 0 to 1.\n"
                f"Safe features:\n{safe_text}"
            )
        return await self._client.respond_json(
            system=(
                "Return only a JSON object with label, summary, and confidence. "
                "Use domains, file_names, semantic_topic, and dominant_family when present. "
                "Do not invent file paths, URLs, or commands."
            ),
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


def _activity_lines(cluster_events_text: str) -> list[str]:
    """Return only rendered event headlines for the no-LLM fallback."""

    if not isinstance(cluster_events_text, str):
        return []
    lines = [line.strip() for line in cluster_events_text.splitlines() if re.match(r"^\d+\.\s+", line.strip())]
    return lines or _safe_feature_lines(cluster_events_text)


def _provider_safe_features(value: str) -> str:
    """Rebuild a fixed allowlist packet before any provider request.

    ``cluster_events_text`` remains a string in the provider protocol for
    backwards compatibility, so this defensive parser is the final protection
    against direct callers supplying evidence or other untrusted text.
    """

    fallback = SafeIntentFeatures(boundary_reasons=["untrusted_input_omitted"])
    if not isinstance(value, str):
        return serialize_safe_features(fallback)
    try:
        payload = json.loads(value)
    except (TypeError, ValueError):
        return serialize_safe_features(fallback)
    if not isinstance(payload, dict):
        return serialize_safe_features(fallback)
    policy_version = payload.get("policy_version")
    if policy_version not in {SAFE_FEATURE_POLICY_VERSION, "safe-intent-features-v1"}:
        return serialize_safe_features(fallback)

    command_families = _safe_string_list(payload.get("command_families"), _SAFE_COMMAND_FAMILIES, 8)
    file_kinds = _safe_string_list(payload.get("file_kinds"), _SAFE_FILE_KINDS, 4)
    domains = _safe_domain_list(payload.get("domains"))
    file_names = _safe_basename_list(payload.get("file_names"))
    dominant_family = payload.get("dominant_family")
    if dominant_family not in _SAFE_EVENT_FAMILIES:
        dominant_family = None
    semantic_topic = _safe_semantic_topic(payload.get("semantic_topic") if isinstance(payload.get("semantic_topic"), str) else None)
    project_key = _safe_project_key(payload.get("project_key") if isinstance(payload.get("project_key"), str) else None)
    event_counts: dict[str, int] = {}
    raw_counts = payload.get("event_counts")
    if isinstance(raw_counts, dict):
        for family, count in raw_counts.items():
            if family not in _SAFE_EVENT_FAMILIES or isinstance(count, bool) or not isinstance(count, int):
                continue
            if count > 0:
                event_counts[family] = min(count, 100_000)
    boundary_reasons = _safe_string_list(payload.get("boundary_reasons"), _SAFE_BOUNDARY_REASONS, 4)
    return serialize_safe_features(SafeIntentFeatures(
        project_key=project_key,
        command_families=command_families,
        file_kinds=file_kinds,
        domains=domains,
        file_names=file_names,
        dominant_family=dominant_family,
        semantic_topic=semantic_topic,
        event_counts=dict(sorted(event_counts.items())),
        duration_seconds=_safe_int(payload.get("duration_seconds"), 86_400),
        child_count=_safe_int(payload.get("child_count"), 1_000),
        boundary_reasons=boundary_reasons or ["untrusted_input_omitted"],
    ))


def _safe_feature_lines(cluster_events_text: str) -> list[str]:
    """Render a generic local fallback summary from a safe feature packet."""

    try:
        payload = json.loads(_provider_safe_features(cluster_events_text))
    except (TypeError, ValueError):  # pragma: no cover - defensive; helper always returns JSON
        return []
    lines: list[str] = []
    for family, count in payload.get("event_counts", {}).items():
        if isinstance(family, str) and isinstance(count, int) and count > 0:
            suffix = "event" if count == 1 else "events"
            lines.append(f"{len(lines) + 1}. Observed {count} {family} {suffix}")
    for family in payload.get("command_families", []):
        if isinstance(family, str):
            lines.append(f"{len(lines) + 1}. Ran {family} commands")
    if not lines:
        lines.append("1. Observed aggregate activity")
    return lines


def _safe_string_list(value: object, allowed: frozenset[str], maximum: int) -> list[str]:
    if not isinstance(value, list):
        return []
    safe: list[str] = []
    for item in value:
        if item in allowed and item not in safe:
            safe.append(item)
        if len(safe) >= maximum:
            break
    return safe


def _safe_int(value: object, maximum: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        return 0
    return min(max(value, 0), maximum)


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


def _safe_domain_roots(domains: list[str]) -> list[str]:
    compact: list[str] = []
    for domain in domains:
        if not isinstance(domain, str):
            continue
        normalized = domain.strip().lower()
        if not normalized or normalized in compact:
            continue
        compact.append(normalized[:120])
        if len(compact) >= 3:
            break
    return compact


def _safe_basenames(values: list[str]) -> list[str]:
    compact: list[str] = []
    for value in values:
        if not isinstance(value, str):
            continue
        basename = _basename(value.strip())
        if not basename or basename in compact:
            continue
        compact.append(basename[:120])
        if len(compact) >= 3:
            break
    return compact


def _safe_domain_list(value: object) -> list[str]:
    if not isinstance(value, list):
        return []
    return _safe_domain_roots([item for item in value if isinstance(item, str)])


def _safe_basename_list(value: object) -> list[str]:
    if not isinstance(value, list):
        return []
    return _safe_basenames([item for item in value if isinstance(item, str)])


def _safe_semantic_topic(value: str | None) -> str | None:
    if not isinstance(value, str):
        return None
    normalized = " ".join(value.split()).strip()
    if not normalized or normalized.casefold() == "unknown":
        return None
    return normalized[:80]


def _safe_project_key(project_tag: str | None) -> str | None:
    if not isinstance(project_tag, str):
        return None
    normalized = project_tag.strip()
    if not normalized:
        return None
    if normalized.startswith("project:"):
        normalized = normalized.split(":", 1)[1].strip()
    return normalized[:120] if normalized else None


def _merge_limited(existing: list[str], additions: list[str], maximum: int) -> list[str]:
    merged = list(existing)
    for item in additions:
        if item and item not in merged:
            merged.append(item)
        if len(merged) >= maximum:
            break
    return merged[:maximum]
