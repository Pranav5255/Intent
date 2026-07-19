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
    TemplateFallbackLabelProvider,
    build_cluster_hints,
    build_parent_hints,
    validate_label_result,
)
from intent_engine.providers import create_label_provider
from intent_engine.normalize import compute_source_hash, normalize_events
from intent_engine.resume import build_resume_payload, merge_resume_payloads
from intent_engine.schemas import DayExport, Intent, IntentInsights, PipelineResult, PipelineWarning
from intent_engine.sessionize import sessionize
from intent_engine.store import IntentStore

PIPELINE_VERSION = "1.0.0"


async def run_pipeline(
    export: DayExport,
    store: IntentStore,
    label_provider: LabelProvider | None = None,
    *,
    force: bool = False,
    logger: DiagnosticsLogger | None = None,
) -> PipelineResult:
    """Process an export deterministically and persist its safe public intent tree."""
    label_provider = label_provider or create_label_provider()
    started_at = time.time()
    normalized = []
    warnings = []
    source_hash = ""
    try:
        normalized, warnings = normalize_events(export.events)
        input_hash = compute_source_hash(normalized)
        source_hash = _provider_cache_hash(input_hash, label_provider.cache_identity)
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
            for cluster_index, cluster in enumerate(await cluster_session(session)):
                child = _cluster_intent(export.date, source_hash, session_index, cluster_index, cluster)
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
                export.date, source_hash, "complete", len(normalized), _duration_ms(started_at), len(warnings)
            )
        return result
    except Exception:
        if logger:
            logger.log_pipeline_run(
                export.date, source_hash, "error", len(normalized), _duration_ms(started_at), len(warnings)
            )
        raise


def _cluster_intent(date: str, source_hash: str, session_index: int, cluster_index: int, cluster) -> Intent:
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
        prefix=_cluster_prefix(cluster),
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
        resume_payload=merge_resume_payloads([child.resume_payload for child in children]),
        children=children,
    )
    validate_intent_tree(parent)
    return parent


async def _label_cluster(provider: LabelProvider, cluster, project_tag: str | None, hints: dict) -> dict:
    text = "\n".join(f"{index}. {event.text}" for index, event in enumerate(cluster, start=1))
    return await _safe_label(provider, "label_cluster", text, project_tag, hints)


async def _label_parent(provider: LabelProvider, children: list[Intent], project_tag: str | None, hints: dict) -> dict:
    text = "\n".join(f"{index}. {child.label}: {child.summary}" for index, child in enumerate(children, start=1))
    return await _safe_label(provider, "label_parent", text, project_tag, hints)


async def _safe_label(
    provider: LabelProvider,
    method: str,
    text: str,
    project_tag: str | None,
    hints: dict | None = None,
) -> dict:
    fallback = TemplateFallbackLabelProvider()
    try:
        return validate_label_result(await getattr(provider, method)(text, project_tag, hints))
    except Exception:
        return validate_label_result(await getattr(fallback, method)(text, project_tag, hints))


def _intent_id(date: str, source_hash: str, scope: str) -> str:
    return str(uuid5(NAMESPACE_URL, f"intent-os/{date}/{source_hash}/{scope}"))


def _cluster_prefix(cluster) -> tuple[str, str, str] | None:
    if len(cluster) < 3:
        return None
    last_three = cluster[-3:]
    final = last_three[-1]
    project = final.entities.project_paths[0] if final.entities.project_paths else ""
    return (last_three[0].family, last_three[1].category, final.entities.command_family or project)


def _provider_cache_hash(input_hash: str, provider_identity: str) -> str:
    return hashlib.sha256(f"{input_hash}:{provider_identity}".encode("utf-8")).hexdigest()[:16]


def _duration_ms(started_at: float) -> int:
    return int((time.time() - started_at) * 1000)
