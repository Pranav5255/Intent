"""Safe stored-intent selection and resume preview construction."""

from __future__ import annotations

import json
import re
from pathlib import PurePath

from pydantic import BaseModel, Field

from intent_engine.llm_base import LLMClient
from intent_engine.providers import create_llm_client, llm_enabled
from intent_engine.schemas import (
    Intent,
    ResumePayload,
    ResumeSelectCandidate,
    ResumeSelectPreview,
    ResumeSelectRequest,
    ResumeSelectResponse,
)
from intent_engine.store import IntentStore

_MAX_CANDIDATES = 10
_QUERY_STOP_WORDS = frozenset({"a", "an", "and", "for", "only", "project", "resume", "the", "this", "to"})


class _RankResponse(BaseModel):
    ranked_ids: list[str] = Field(default_factory=list)


async def select_resume_preview(
    store: IntentStore,
    request: ResumeSelectRequest,
    *,
    client: LLMClient | None = None,
) -> ResumeSelectResponse | None:
    """Resolve only persisted intents and return a preview; never restores anything."""

    intents = await store.get_root_intents()
    if request.intent_id:
        exact = await store.get_intent_by_id(request.intent_id.strip())
        if exact is None:
            return None
        intents = [exact]

    if request.project_tag:
        expected_tag = _normalized_project_tag(request.project_tag)
        intents = [intent for intent in intents if expected_tag in {_normalized_project_tag(tag) for tag in intent.tags}]

    ranked = _rank_deterministically(intents, request.query)
    if request.query:
        ranked = await _rerank_known_candidates(ranked, request.query, client)
    ranked = ranked[:_MAX_CANDIDATES]
    if not ranked:
        return None

    candidates = [_candidate(intent, score) for intent, score in ranked]
    selected_index = _selected_index(request, ranked)
    if selected_index is None:
        return ResumeSelectResponse(needs_picker=True, candidates=candidates)

    intent, score = ranked[selected_index]
    return ResumeSelectResponse(
        needs_picker=False,
        candidates=candidates,
        selected=_preview(intent, score, request.restore_scope),
    )


def _rank_deterministically(intents: list[Intent], query: str | None) -> list[tuple[Intent, float]]:
    if not query:
        return [(intent, 1.0) for intent in intents]
    terms = [term for term in re.findall(r"[a-z0-9]+", query.casefold()) if term not in _QUERY_STOP_WORDS]
    if not terms:
        return [(intent, 1.0) for intent in intents]
    scored: list[tuple[Intent, float]] = []
    for intent in intents:
        haystack = " ".join([intent.label, intent.summary, *intent.tags]).casefold()
        score = float(sum(haystack.count(term) for term in terms))
        if score:
            scored.append((intent, score))
    return sorted(scored, key=lambda item: (-item[1], -item[0].end_ts, item[0].id))


async def _rerank_known_candidates(
    ranked: list[tuple[Intent, float]], query: str, client: LLMClient | None,
) -> list[tuple[Intent, float]]:
    if not ranked or not llm_enabled():
        return ranked
    try:
        active_client = client or create_llm_client()
        supplied_ids = [intent.id for intent, _score in ranked]
        response = await active_client.respond_json(
            system=(
                "Rank only the supplied stored intent IDs for the user's resume query. "
                "Return every supplied ID once, most relevant first. Do not invent IDs or restore actions."
            ),
            user=json.dumps({
                "query": query,
                "candidates": [
                    {"intent_id": intent.id, "label": intent.label, "summary": intent.summary, "tags": intent.tags}
                    for intent, _score in ranked
                ],
            }, separators=(",", ":")),
            schema_name="resume_candidate_ranking",
            schema=_RankResponse.model_json_schema(),
        )
        ordered_ids = _RankResponse.model_validate(response).ranked_ids
        if len(ordered_ids) != len(supplied_ids) or set(ordered_ids) != set(supplied_ids):
            return ranked
        scores = {intent.id: score for intent, score in ranked}
        by_id = {intent.id: intent for intent, _score in ranked}
        return [(by_id[intent_id], scores[intent_id]) for intent_id in ordered_ids]
    except Exception:
        return ranked


def _selected_index(request: ResumeSelectRequest, ranked: list[tuple[Intent, float]]) -> int | None:
    if request.intent_id or request.project_tag:
        return 0 if len(ranked) == 1 else None
    if len(ranked) == 1:
        return 0
    return 0 if ranked[0][1] >= 2 * ranked[1][1] else None


def _candidate(intent: Intent, score: float) -> ResumeSelectCandidate:
    return ResumeSelectCandidate(
        intent_id=intent.id,
        label=intent.label,
        summary=intent.summary,
        project_tag=_project_tag(intent),
        workspace_root=_workspace_root(intent),
        score=score,
    )


def _preview(intent: Intent, score: float, restore_scope: str | None) -> ResumeSelectPreview:
    root = _workspace_root(intent)
    payload = intent.resume_payload
    if restore_scope == "same_project":
        payload = _scope_payload(payload, root)
    return ResumeSelectPreview(**_candidate(intent, score).model_dump(), resume_payload=payload)


def _project_tag(intent: Intent) -> str | None:
    return next((tag for tag in intent.tags if tag.casefold().startswith("project:")), None)


def _normalized_project_tag(value: str) -> str:
    normalized = value.strip().casefold()
    return normalized if normalized.startswith("project:") else f"project:{normalized}"


def _workspace_root(intent: Intent) -> str | None:
    if intent.semantic and intent.semantic.workspace_root:
        return intent.semantic.workspace_root
    cwd = intent.resume_payload.shell.get("cwd")
    if isinstance(cwd, str) and cwd and all(_is_within(path, cwd) for path in intent.resume_payload.files):
        return cwd
    if not intent.resume_payload.files:
        return None
    parents = {_parent(path) for path in intent.resume_payload.files}
    return next(iter(parents)) if len(parents) == 1 else None


def _scope_payload(payload: ResumePayload, root: str | None) -> ResumePayload:
    if not root:
        return ResumePayload(urls=payload.urls)
    files = [path for path in payload.files if _is_within(path, root)]
    cwd = payload.shell.get("cwd")
    shell = dict(payload.shell) if isinstance(cwd, str) and _is_within(cwd, root) else {}
    return ResumePayload(files=files, urls=payload.urls, shell=shell)


def _is_within(path: str, root: str) -> bool:
    value = path.replace("\\", "/").rstrip("/").casefold()
    base = root.replace("\\", "/").rstrip("/").casefold()
    return value == base or value.startswith(f"{base}/")


def _parent(path: str) -> str:
    return str(PurePath(path.replace("\\", "/")).parent)
