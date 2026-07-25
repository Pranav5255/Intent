"""Deterministic Phase 1 orchestration from Role A exports to stored intents."""

from __future__ import annotations

import hashlib
import time
from uuid import NAMESPACE_URL, uuid5

from intent_engine.cluster import cluster_session
from intent_engine.enrich import aggregate_stats, compute_insight_browser, compute_insight_editor, compute_insight_shell, compute_stats, derive_project_tag, detect_todos, validate_intent_tree
from intent_engine.logging import DiagnosticsLogger
from intent_engine.labeling import (
    LabelProvider,
    SAFE_FEATURE_POLICY_VERSION,
    TemplateFallbackLabelProvider,
    build_cluster_hints,
    build_parent_hints,
    build_safe_cluster_features,
    build_safe_parent_features,
    safe_provider_hints,
    serialize_safe_features,
    validate_label_result,
)
from intent_engine.providers import create_label_provider, create_semantic_llm_client, semantic_clustering_enabled
from intent_engine.normalize import compute_source_hash, normalize_events
from intent_engine.resume import RESUME_POLICY_VERSION, build_resume_payload
from intent_engine.schemas import DayExport, Intent, IntentInsights, PipelineResult, PipelineWarning, SemanticIntentMetadata
from intent_engine.semantic_cluster import SemanticRefinementResult, refine_semantic_clusters_detailed, semantic_cache_identity
from intent_engine.llm_base import LLMClient
from intent_engine.sessionize import sessionize
from intent_engine.store import IntentStore

PIPELINE_VERSION = "1.1.0"


async def run_pipeline(
    export: DayExport,
    store: IntentStore,
    label_provider: LabelProvider | None = None,
    *,
    force: bool = False,
    logger: DiagnosticsLogger | None = None,
    semantic_client: LLMClient | None = None,
) -> PipelineResult:
    """Process an export deterministically and persist its safe public intent tree."""
    label_provider = label_provider or create_label_provider()
    started_at = time.time()
    normalized = []
    warnings = []
    source_hash = ""
    semantic_enabled = semantic_clustering_enabled()
    semantic_identity: str | None = None
    semantic_fallback_reason: str | None = None
    try:
        normalized, warnings = normalize_events(export.events)
        input_hash = compute_source_hash(normalized)
        if semantic_enabled:
            try:
                semantic_client = semantic_client or create_semantic_llm_client()
                semantic_identity = semantic_cache_identity(semantic_client)
            except Exception:
                semantic_fallback_reason = "provider_unavailable"
                semantic_identity = _semantic_unavailable_identity()
        cache_identity = label_provider.cache_identity
        if semantic_enabled:
            cache_identity = f"{cache_identity}:{semantic_identity}"
        source_hash = _provider_cache_hash(input_hash, cache_identity)
        if not force:
            cached_intents = await store.get_cached_intents(export.date, source_hash)
            if cached_intents is not None:
                cached_warnings = await store.get_cached_warnings(export.date, source_hash) or []
                if logger:
                    logger.log_cache_hit(export.date, source_hash)
                return PipelineResult(
                    intents=cached_intents,
                    warnings=[PipelineWarning.model_validate(item) for item in cached_warnings],
                    source_hash=source_hash,
                    pipeline_version=PIPELINE_VERSION,
                    cached=True,
                )

        intents: list[Intent] = []
        for session_index, session in enumerate(await sessionize(normalized)):
            children: list[Intent] = []
            child_command_families: list[str] = []
            deterministic_clusters = await cluster_session(session)
            semantic_result: SemanticRefinementResult | None = None
            clusters = deterministic_clusters
            if semantic_enabled and semantic_client is not None:
                semantic_result = await refine_semantic_clusters_detailed(session, semantic_client)
                if semantic_result.clusters is None:
                    semantic_fallback_reason = semantic_result.fallback_reason or "invalid_response"
                    warnings.append(PipelineWarning(
                        level="warning",
                        message=f"Semantic refinement fallback: {semantic_fallback_reason}",
                    ))
                else:
                    clusters = semantic_result.clusters
                    semantic_identity = semantic_result.provider_identity or semantic_identity
            for cluster_index, cluster in enumerate(clusters):
                metadata = _cluster_semantic_metadata(cluster, semantic_result)
                child = _cluster_intent(export.date, source_hash, session_index, cluster_index, cluster, metadata)
                cluster_hints = build_cluster_hints(cluster)
                command_family = cluster_hints.get("command_family")
                if isinstance(command_family, str) and command_family:
                    child_command_families.append(command_family)
                label = await _label_cluster(
                    label_provider,
                    cluster,
                    child.tags[0] if child.tags else None,
                    cluster_hints,
                )
                children.append(child.model_copy(update=label))
            if len(children) == 1:
                intents.append(children[0].model_copy(update={"parent_id": None, "depth": 0}))
            elif children:
                parent = _session_intent(export.date, source_hash, session_index, session, children)
                parent_hints = build_parent_hints(
                    child_command_families,
                    parent.tags[0] if parent.tags else None,
                )
                label = await _label_parent(label_provider, children, parent.tags[0] if parent.tags else None, parent_hints)
                intents.append(parent.model_copy(update=label))

        result = PipelineResult(intents=intents, warnings=warnings, source_hash=source_hash, pipeline_version=PIPELINE_VERSION)
        await store.save_pipeline_run(export.date, result)
        if logger:
            logger.log_pipeline_run(
                export.date,
                source_hash,
                "complete",
                len(normalized),
                _duration_ms(started_at),
                len(warnings),
                semantic_provider_identity=semantic_identity,
                semantic_fallback_reason=semantic_fallback_reason,
            )
        return result
    except Exception:
        if logger:
            logger.log_pipeline_run(
                export.date,
                source_hash,
                "error",
                len(normalized),
                _duration_ms(started_at),
                len(warnings),
                semantic_provider_identity=semantic_identity,
                semantic_fallback_reason=semantic_fallback_reason,
            )
        raise


def _cluster_intent(
    date: str, source_hash: str, session_index: int, cluster_index: int, cluster,
    semantic: SemanticIntentMetadata | None = None,
) -> Intent:
    parent_id = _intent_id(date, source_hash, f"session:{session_index}")
    tag = derive_project_tag(cluster)
    stats = compute_stats(cluster)
    return Intent(
        id=_intent_id(date, source_hash, f"session:{session_index}:cluster:{cluster_index}"),
        parent_id=parent_id,
        date=date,
        label="Work Task",
        summary="Inferred work session",
        start_ts=cluster[0].ts,
        end_ts=cluster[-1].ts,
        depth=1,
        tags=[tag] if tag else [],
        stats=stats,
        insights=IntentInsights(
            editor=compute_insight_editor(cluster),
            browser=compute_insight_browser(cluster),
            shell=compute_insight_shell(cluster),
        ),
        resume_payload=build_resume_payload(cluster),
        todos=detect_todos(cluster),
        evidence=[item for event in cluster for item in event.evidence],
        prefix=_cluster_prefix(cluster),
        semantic=semantic,
        privacy_policy_version=SAFE_FEATURE_POLICY_VERSION,
    )


def _session_intent(date: str, source_hash: str, session_index: int, session, children: list[Intent]) -> Intent:
    stats = aggregate_stats(children)
    parent = Intent(
        id=_intent_id(date, source_hash, f"session:{session_index}"),
        date=date,
        label="Work Session",
        summary="Inferred work session",
        start_ts=session[0].ts,
        end_ts=session[-1].ts,
        depth=0,
        tags=list(dict.fromkeys(tag for child in children for tag in child.tags)),
        stats=stats,
        insights=IntentInsights(),
        # A parent is the user-facing "open this session" action. Rebuild its
        # payload from the full chronology so each observed browser tab keeps
        # its final URL rather than a union of navigation history from children.
        resume_payload=build_resume_payload(session),
        evidence=[item for child in children for item in child.evidence],
        children=children,
        semantic=_aggregate_semantic_metadata(children),
        privacy_policy_version=SAFE_FEATURE_POLICY_VERSION,
    )
    validate_intent_tree(parent)
    return parent


async def _label_cluster(provider: LabelProvider, cluster, project_tag: str | None, hints: dict) -> dict:
    text = serialize_safe_features(build_safe_cluster_features(cluster))
    return await _safe_label(provider, "label_cluster", text, project_tag, hints)


async def _label_parent(provider: LabelProvider, children: list[Intent], project_tag: str | None, hints: dict) -> dict:
    text = serialize_safe_features(build_safe_parent_features(children))
    return await _safe_label(provider, "label_parent", text, project_tag, hints)


async def _safe_label(
    provider: LabelProvider,
    method: str,
    text: str,
    project_tag: str | None,
    hints: dict | None = None,
) -> dict:
    fallback = TemplateFallbackLabelProvider()
    is_local_template = isinstance(provider, TemplateFallbackLabelProvider)
    provider_project_tag = project_tag if is_local_template else None
    provider_hints = hints if is_local_template else safe_provider_hints(hints)
    try:
        return validate_label_result(await getattr(provider, method)(text, provider_project_tag, provider_hints))
    except Exception:
        return validate_label_result(await getattr(fallback, method)(text, project_tag, hints))


def _intent_id(date: str, source_hash: str, scope: str) -> str:
    return str(uuid5(NAMESPACE_URL, f"intent/{date}/{source_hash}/{scope}"))


def _cluster_prefix(cluster) -> tuple[str, str, str] | None:
    if len(cluster) < 3:
        return None
    last_three = cluster[-3:]
    final = last_three[-1]
    project = final.entities.project_paths[0] if final.entities.project_paths else ""
    return (last_three[0].family, last_three[1].category, final.entities.command_family or project)


def _provider_cache_hash(input_hash: str, provider_identity: str) -> str:
    return hashlib.sha256(
        f"{input_hash}:{provider_identity}:{SAFE_FEATURE_POLICY_VERSION}:{RESUME_POLICY_VERSION}".encode("utf-8")
    ).hexdigest()[:16]


def _semantic_unavailable_identity() -> str:
    from intent_engine.providers import SEMANTIC_CONTENT_POLICY_VERSION
    from intent_engine.semantic_cluster import SEMANTIC_CLUSTER_POLICY_VERSION
    import os

    provider = os.environ.get("LLM_PROVIDER", "openai").strip().lower() or "openai"
    model = os.environ.get("INTENT_LLM_MODEL", "").strip() or "configured-default"
    return f"semantic:{provider}:{model}:content-policy-{SEMANTIC_CONTENT_POLICY_VERSION}:cluster-policy-{SEMANTIC_CLUSTER_POLICY_VERSION}"


def _cluster_semantic_metadata(cluster, result: SemanticRefinementResult | None) -> SemanticIntentMetadata | None:
    if result is None or result.clusters is None or not result.proposals:
        return None
    proposals = [result.proposals[event.id] for event in cluster if event.id in result.proposals]
    if not proposals:
        return None
    roles: dict[str, int] = {}
    topics: dict[str, int] = {}
    for proposal in proposals:
        roles[proposal.role] = roles.get(proposal.role, 0) + 1
        topic = proposal.topic.strip()
        if topic and topic.casefold() != "unknown":
            topics[topic] = topics.get(topic, 0) + 1
    roots = {path for event in cluster for path in event.entities.project_paths}
    return SemanticIntentMetadata(
        confidence=sum(proposal.confidence for proposal in proposals) / len(proposals),
        event_roles=roles,
        topic=max(topics, key=topics.get) if topics else None,
        workspace_root=next(iter(roots)) if len(roots) == 1 else None,
        provider_identity=result.provider_identity,
    )


def _aggregate_semantic_metadata(children: list[Intent]) -> SemanticIntentMetadata | None:
    metadata = [child.semantic for child in children if child.semantic is not None]
    if not metadata:
        return None
    roles: dict[str, int] = {}
    for item in metadata:
        for role, count in item.event_roles.items():
            roles[role] = roles.get(role, 0) + count
    confidences = [item.confidence for item in metadata if item.confidence is not None]
    roots = {item.workspace_root for item in metadata if item.workspace_root}
    providers = {item.provider_identity for item in metadata if item.provider_identity}
    topics = {item.topic for item in metadata if item.topic}
    return SemanticIntentMetadata(
        confidence=sum(confidences) / len(confidences) if confidences else None,
        event_roles=roles,
        topic=next(iter(topics)) if len(topics) == 1 else None,
        workspace_root=next(iter(roots)) if len(roots) == 1 else None,
        provider_identity=next(iter(providers)) if len(providers) == 1 else None,
    )


def _duration_ms(started_at: float) -> int:
    return int((time.time() - started_at) * 1000)
