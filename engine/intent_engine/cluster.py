"""Deterministic, topic-aware grouping of a single activity session."""

from __future__ import annotations

from collections import Counter
from dataclasses import dataclass

from intent_engine.schemas import NormalizedEvent


@dataclass
class _Run:
    events: list[NormalizedEvent]
    boundary_before: str | None = None


class ClusterEngine:
    """Build bounded chronological sub-intents without external inference."""

    def __init__(self) -> None:
        self.run_groups: list[list[NormalizedEvent]] = []

    def _get_topic_score(self, events: list[NormalizedEvent]) -> dict:
        """Summarize the strongest project, command, file, and domain signals."""

        projects = [path for event in events for path in event.entities.project_paths]
        commands = [event.entities.command_family for event in events if event.entities.command_family]
        files = [event.entities.file_name for event in events if event.entities.file_name]
        domains = [event.entities.domain for event in events if event.entities.domain]
        context_terms = [term for event in events for term in event.entities.context_terms]
        project, project_score = _most_common(projects)
        command_family, command_score = _most_common(commands)
        top_file, file_score = _most_common(files)
        top_domain, domain_score = _most_common(domains)
        return {
            "project": project,
            "project_score": project_score,
            "command_family": command_family,
            "command_score": command_score,
            "file_names": _unique(files),
            "top_file": top_file,
            "file_score": file_score,
            "domains": _unique(domains),
            "top_domain": top_domain,
            "domain_score": domain_score,
            "context_terms": _unique(context_terms),
        }

    def _time_adjacent(
        self, event1: NormalizedEvent, event2: NormalizedEvent, max_gap_minutes: int = 5
    ) -> bool:
        return event2.ts - event1.ts <= max_gap_minutes * 60

    def _topic_shift_strong(self, topic1: dict, topic2: dict) -> bool:
        project_differs = _populated_different(topic1.get("project"), topic2.get("project"))
        command_differs = _populated_different(topic1.get("command_family"), topic2.get("command_family"))
        file_differs = _populated_different(topic1.get("top_file"), topic2.get("top_file"))
        return (project_differs and (command_differs or file_differs)) or command_differs

    async def cluster_session(self, session: list[NormalizedEvent]) -> list[list[NormalizedEvent]]:
        """Return up to four chronological, non-overlapping topic clusters."""

        if not session:
            self.run_groups = []
            return []

        runs: list[_Run] = []
        current_run = _Run(events=[session[0]])
        previous_topic = self._get_topic_score(current_run.events)

        for event in session[1:]:
            event_topic = self._get_topic_score([event])
            boundary = self._boundary_reason(current_run.events, previous_topic, event, event_topic)
            if boundary is None:
                current_run.events.append(event)
                previous_topic = self._get_topic_score(current_run.events)
                continue

            runs.append(current_run)
            current_run = _Run(events=[event], boundary_before=boundary)
            previous_topic = event_topic

        runs.append(current_run)
        runs = self._merge_gap_runs(runs)
        runs = self._cap_runs(runs)
        clusters = [run.events for run in runs]
        self._validate_invariants(session, clusters)
        self.run_groups = clusters
        return clusters

    def _boundary_reason(
        self,
        current_events: list[NormalizedEvent],
        current_topic: dict,
        event: NormalizedEvent,
        event_topic: dict,
    ) -> str | None:
        if not self._time_adjacent(current_events[-1], event):
            return "gap"
        if self._starts_command_phase(current_events, event):
            return "command_phase"
        if self._topic_shift_strong(current_topic, event_topic):
            return "topic_shift"
        return None

    @staticmethod
    def _starts_command_phase(current_events: list[NormalizedEvent], event: NormalizedEvent) -> bool:
        command_family = event.entities.command_family
        if event.family != "command" or not command_family:
            return False
        meaningful_events = [item for item in current_events if item.family not in {"focus", "idle"}]
        if len(meaningful_events) < 2:
            return False
        active_families = [item.entities.command_family for item in current_events if item.entities.command_family]
        return not active_families or active_families[-1] != command_family

    def _merge_gap_runs(self, runs: list[_Run]) -> list[_Run]:
        if len(runs) < 2:
            return runs

        merged: list[_Run] = [runs[0]]
        for run in runs[1:]:
            previous = merged[-1]
            similarity = self._topic_similarity(self._get_topic_score(previous.events), self._get_topic_score(run.events))
            if run.boundary_before == "gap" and similarity >= 0.7:
                previous.events.extend(run.events)
            else:
                merged.append(run)
        return merged

    def _cap_runs(self, runs: list[_Run]) -> list[_Run]:
        while len(runs) > 4:
            smallest_index = min(range(len(runs)), key=lambda index: (len(runs[index].events), index))
            neighbor_indexes = [index for index in (smallest_index - 1, smallest_index + 1) if 0 <= index < len(runs)]
            target_index = max(
                neighbor_indexes,
                key=lambda index: (
                    self._topic_similarity(
                        self._get_topic_score(runs[smallest_index].events), self._get_topic_score(runs[index].events)
                    ),
                    -index,
                ),
            )
            left, right = sorted((smallest_index, target_index))
            runs[left : right + 1] = [_Run(events=runs[left].events + runs[right].events, boundary_before=runs[left].boundary_before)]
        return runs

    def _topic_similarity(self, topic1: dict, topic2: dict) -> float:
        comparisons: list[tuple[float, bool | None]] = [
            (0.4, _values_match(topic1.get("project"), topic2.get("project"))),
            (0.3, _values_match(topic1.get("command_family"), topic2.get("command_family"))),
            (0.2, _values_match(topic1.get("top_file"), topic2.get("top_file"))),
        ]
        domain_comparable = bool(topic1.get("domains") and topic2.get("domains"))
        domain_matches = bool(set(topic1.get("domains", [])) & set(topic2.get("domains", [])))
        comparisons.append((0.1, domain_matches if domain_comparable else None))
        context_comparable = bool(topic1.get("context_terms") and topic2.get("context_terms"))
        context_matches = bool(set(topic1.get("context_terms", [])) & set(topic2.get("context_terms", [])))
        comparisons.append((0.15, context_matches if context_comparable else None))

        available = [(weight, matched) for weight, matched in comparisons if matched is not None]
        if not available:
            return 0.0
        total_weight = sum(weight for weight, _ in available)
        return sum(weight for weight, matched in available if matched) / total_weight

    @staticmethod
    def _validate_invariants(session: list[NormalizedEvent], clusters: list[list[NormalizedEvent]]) -> None:
        assert all(cluster for cluster in clusters), "clusters must not be empty"
        flattened = [event for cluster in clusters for event in cluster]
        assert [id(event) for event in flattened] == [id(event) for event in session], "events must appear exactly once"
        assert all(
            event1.ts <= event2.ts for cluster in clusters for event1, event2 in zip(cluster, cluster[1:])
        ), "events in a cluster must be chronological"
        assert all(
            clusters[index][-1].ts <= clusters[index + 1][0].ts for index in range(len(clusters) - 1)
        ), "clusters must be chronological"


async def cluster_session(session: list[NormalizedEvent]) -> list[list[NormalizedEvent]]:
    """Convenience entry point for clustering one session without retained state."""

    return await ClusterEngine().cluster_session(session)


def _most_common(values: list[str]) -> tuple[str | None, int]:
    if not values:
        return None, 0
    counts = Counter(values)
    value = max(counts, key=counts.get)
    return value, counts[value]


def _unique(values: list[str]) -> list[str]:
    return list(dict.fromkeys(values))


def _populated_different(value1: object, value2: object) -> bool:
    return bool(value1 and value2 and value1 != value2)


def _values_match(value1: object, value2: object) -> bool | None:
    if not value1 or not value2:
        return None
    return value1 == value2
