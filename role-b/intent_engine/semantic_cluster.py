"""Advisory semantic clustering backed by the existing cloud LLM client."""

from __future__ import annotations

import asyncio
import json
import os
from dataclasses import dataclass
from typing import Literal

from pydantic import ValidationError

from intent_engine.llm import LLMError
from intent_engine.llm_base import LLMClient
from intent_engine.providers import (
    SEMANTIC_CONTENT_POLICY_VERSION,
    create_llm_client,
    semantic_clustering_enabled,
    semantic_timeout_ms,
)
from intent_engine.schemas import NormalizedEvent, SemanticEventProposal, SemanticProposalResponse
from intent_engine.semantic_pack import SemanticPacketEvent, build_semantic_candidate_packets


MIN_LINK_CONFIDENCE = 0.70
SEMANTIC_CLUSTER_POLICY_VERSION = "1"

_SYSTEM_PROMPT = (
    "Classify every supplied event. Return exactly one proposal per event_id. "
    "Use task or supporting_context only for work-related activity, background for ambient media, "
    "and unrelated otherwise. Link only supplied event IDs. Never invent files, URLs, commands, "
    "or restore actions."
)


@dataclass(frozen=True)
class SemanticRefinementResult:
    """Validated semantic output and safe provenance for pipeline integration."""

    clusters: list[list[NormalizedEvent]] | None
    proposals: dict[str, SemanticEventProposal]
    provider_identity: str | None
    fallback_reason: Literal["disabled", "provider_unavailable", "timeout", "invalid_response"] | None = None


async def refine_semantic_clusters(
    session: list[NormalizedEvent], client: LLMClient | None = None
) -> list[list[NormalizedEvent]] | None:
    """Return validated semantic clusters, or ``None`` to use deterministic clustering."""

    result = await refine_semantic_clusters_detailed(session, client)
    return result.clusters


async def refine_semantic_clusters_detailed(
    session: list[NormalizedEvent], client: LLMClient | None = None
) -> SemanticRefinementResult:
    """Return semantic clusters with safe provenance or a normalized fallback signal."""

    if not semantic_clustering_enabled():
        return SemanticRefinementResult(None, {}, None, "disabled")

    packets = build_semantic_candidate_packets(session)
    packet_events = [event for packet in packets for event in packet.events]
    if not packet_events:
        return SemanticRefinementResult([], {}, None)
    if len({event.event_id for event in packet_events}) != len(packet_events):
        return SemanticRefinementResult(None, {}, None, "invalid_response")

    llm_client: LLMClient | None = None
    try:
        llm_client = client or create_llm_client()
        response = await asyncio.wait_for(
            llm_client.respond_json(
                system=_SYSTEM_PROMPT,
                user=_packet_prompt(packets),
                schema_name="semantic_cluster_proposals",
                schema=SemanticProposalResponse.model_json_schema(),
            ),
            timeout=semantic_timeout_ms() / 1000,
        )
        proposals = SemanticProposalResponse.model_validate(response).proposals
        clusters = _validated_clusters(session, packet_events, proposals)
        if clusters is None:
            return SemanticRefinementResult(None, {}, semantic_cache_identity(llm_client), "invalid_response")
        return SemanticRefinementResult(
            clusters,
            {proposal.event_id: proposal for proposal in proposals},
            semantic_cache_identity(llm_client),
        )
    except asyncio.TimeoutError:
        return SemanticRefinementResult(None, {}, semantic_cache_identity(llm_client) if llm_client else None, "timeout")
    except ValidationError:
        return SemanticRefinementResult(None, {}, semantic_cache_identity(llm_client) if llm_client else None, "invalid_response")
    except LLMError as exc:
        reason = "timeout" if "timed out" in str(exc).lower() else "provider_unavailable"
        return SemanticRefinementResult(None, {}, semantic_cache_identity(llm_client) if llm_client else None, reason)
    except Exception:
        return SemanticRefinementResult(None, {}, semantic_cache_identity(llm_client) if llm_client else None, "provider_unavailable")


def semantic_cache_identity(client: LLMClient) -> str:
    """Return the semantic provider identity for future pipeline cache keys."""

    provider = os.environ.get("LLM_PROVIDER", "openai").strip().lower() or "openai"
    model = str(getattr(client, "model", "unknown"))
    return (
        f"semantic:{provider}:{model}:content-policy-{SEMANTIC_CONTENT_POLICY_VERSION}:"
        f"cluster-policy-{SEMANTIC_CLUSTER_POLICY_VERSION}"
    )


def _packet_prompt(packets) -> str:
    return json.dumps(
        {"packets": [packet.model_dump(mode="json") for packet in packets]},
        separators=(",", ":"),
    )


def _validated_clusters(
    session: list[NormalizedEvent],
    packet_events: list[SemanticPacketEvent],
    proposals: list[SemanticEventProposal],
) -> list[list[NormalizedEvent]] | None:
    supplied_ids = [event.event_id for event in packet_events]
    proposal_ids = [proposal.event_id for proposal in proposals]
    if len(proposal_ids) != len(supplied_ids) or set(proposal_ids) != set(supplied_ids):
        return None

    session_by_id = {event.id: event for event in session}
    if any(event_id not in session_by_id for event_id in supplied_ids):
        return None

    packet_by_id = {event.event_id: event for event in packet_events}
    proposal_by_id = {proposal.event_id: proposal for proposal in proposals}
    union_find = _UnionFind(supplied_ids, session_by_id)

    for event_id in supplied_ids:
        packet_event = packet_by_id[event_id]
        proposal = proposal_by_id[event_id]
        if packet_event.deterministic_role == "background" or not _is_linkable(proposal):
            continue
        for linked_event_id in proposal.linked_event_ids or []:
            if linked_event_id == event_id:
                continue
            linked_packet_event = packet_by_id.get(linked_event_id)
            linked_proposal = proposal_by_id.get(linked_event_id)
            if linked_packet_event is None or linked_proposal is None:
                return None
            if linked_packet_event.deterministic_role == "background" or not _is_linkable(linked_proposal):
                continue
            union_find.union_if_workspace_safe(event_id, linked_event_id)

    grouped: dict[str, list[NormalizedEvent]] = {}
    for event_id in supplied_ids:
        grouped.setdefault(union_find.find(event_id), []).append(session_by_id[event_id])
    clusters = [sorted(events, key=lambda event: (event.ts, event.ordinal)) for events in grouped.values()]
    return sorted(clusters, key=lambda events: (events[0].ts, events[0].ordinal))


def _is_linkable(proposal: SemanticEventProposal) -> bool:
    return proposal.confidence >= MIN_LINK_CONFIDENCE and proposal.role in {"task", "supporting_context"}


class _UnionFind:
    def __init__(self, event_ids: list[str], events: dict[str, NormalizedEvent]) -> None:
        self.parent = {event_id: event_id for event_id in event_ids}
        self.workspace_roots = {
            event_id: set(events[event_id].entities.project_paths)
            for event_id in event_ids
        }

    def find(self, event_id: str) -> str:
        parent = self.parent[event_id]
        if parent != event_id:
            self.parent[event_id] = self.find(parent)
        return self.parent[event_id]

    def union_if_workspace_safe(self, first: str, second: str) -> None:
        first_root = self.find(first)
        second_root = self.find(second)
        if first_root == second_root:
            return
        first_workspaces = self.workspace_roots[first_root]
        second_workspaces = self.workspace_roots[second_root]
        if first_workspaces and second_workspaces and first_workspaces.isdisjoint(second_workspaces):
            return
        self.parent[second_root] = first_root
        self.workspace_roots[first_root] = first_workspaces | second_workspaces
        self.workspace_roots.pop(second_root, None)
