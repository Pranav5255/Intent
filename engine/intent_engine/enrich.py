"""Deterministic stats, insights, and project tags for intent clusters."""

from __future__ import annotations

from collections import Counter
import re

from intent_engine.schemas import Intent, IntentStats, NormalizedEvent, TodoObservation


def derive_project_tag(cluster: list[NormalizedEvent]) -> str | None:
    """Vote for a project name using normalized paths only."""

    candidates: list[str] = []
    for event in cluster:
        candidates.extend(event.entities.project_paths)
        if event.entities.cwd:
            candidates.append(event.entities.cwd)
        if event.family == "editor" and event.entities.file_path:
            parent = _parent_path(event.entities.file_path)
            if parent:
                candidates.append(parent)

    segments = [segment for path in candidates if (segment := _final_segment(path))]
    if not segments:
        return None
    return f"project:{_first_most_common(segments)}"


def compute_stats(cluster: list[NormalizedEvent]) -> IntentStats:
    """Compute stable aggregate statistics without host or raw-event access."""

    if not cluster:
        return IntentStats(event_count=0, duration_seconds=0)
    sources = Counter(event.source for event in cluster)
    return IntentStats(
        event_count=len(cluster),
        duration_seconds=cluster[-1].ts - cluster[0].ts,
        sources=dict(sources),
        unique_apps=list(dict.fromkeys(event.source for event in cluster)),
    )


def aggregate_stats(children: list[Intent]) -> IntentStats:
    """Aggregate persisted child intent statistics in child order."""

    event_count = 0
    duration_seconds = 0
    sources: dict[str, int] = {}
    unique_apps: list[str] = []
    seen_apps: set[str] = set()
    for child in children:
        event_count += child.stats.event_count
        duration_seconds += child.stats.duration_seconds
        for source, count in child.stats.sources.items():
            sources[source] = sources.get(source, 0) + count
        for app in child.stats.unique_apps:
            if app not in seen_apps:
                seen_apps.add(app)
                unique_apps.append(app)
    return IntentStats(
        event_count=event_count,
        duration_seconds=duration_seconds,
        sources=sources,
        unique_apps=unique_apps,
    )


def validate_intent_tree(root: Intent) -> bool:
    """Assert invariants for a depth-zero intent and its direct children."""

    assert root.depth == 0, "root intent must have depth 0"
    assert all(child.depth == 1 for child in root.children), "children must have depth 1"
    assert all(child.parent_id == root.id for child in root.children), "children must reference root"
    assert sum(child.stats.event_count for child in root.children) == root.stats.event_count, "event counts must aggregate"
    assert all(root.start_ts <= child.start_ts for child in root.children), "root starts before children"
    assert all(root.end_ts >= child.end_ts for child in root.children), "root ends after children"
    return True


def compute_insight_editor(cluster: list[NormalizedEvent]) -> list[dict]:
    """Summarize editor activity without exposing document text."""

    editor_events = [event for event in cluster if event.family == "editor"]
    if not editor_events:
        return []

    typed_chars = sum(event.signals.typed_chars for event in editor_events)
    file_names = [event.entities.file_name for event in editor_events if event.entities.file_name]
    dominant_file = _first_most_common(file_names) if file_names else None
    saves = sum(
        event.signals.save and event.entities.file_name == dominant_file for event in editor_events
    ) if dominant_file else 0
    return [{"file": dominant_file, "typed_chars": typed_chars, "saves": saves}]


def detect_todos(cluster: list[NormalizedEvent]) -> list[TodoObservation]:
    """Extract safe TODO observations only from inserted document changes."""

    observations: list[TodoObservation] = []
    marker_pattern = re.compile(r"\b(TODO|FIXME|XXX)\b", re.IGNORECASE)
    for event in cluster:
        if event.family != "editor" or event.category != "document_change" or not event.signals.todo_added:
            continue
        if not event.entities.file_path:
            continue
        match = marker_pattern.search(event.text or "")
        marker = match.group(1).upper() if match else "TODO"
        observations.append(TodoObservation(path=event.entities.file_path, observed_ts=event.ts, marker=marker))
    return observations


def compute_insight_browser(cluster: list[NormalizedEvent]) -> list[dict]:
    """Return the three most-visited browser domains from normalized metadata."""

    domains = [event.entities.domain for event in cluster if event.family == "browser" and event.entities.domain]
    if not domains:
        return []

    counts = Counter(domains)
    first_seen = {domain: index for index, domain in enumerate(dict.fromkeys(domains))}
    documentation_domains = {domain for domain in counts if "doc" in domain.lower() or "docs" in domain.lower()}
    repeated_searches = {domain for domain, count in counts.items() if count > 1}
    del documentation_domains, repeated_searches  # Calculated locally; public records intentionally remain compact.
    ordered = sorted(counts, key=lambda domain: (-counts[domain], first_seen[domain]))[:3]
    return [{"domain": domain, "visits": counts[domain]} for domain in ordered]


def compute_insight_shell(cluster: list[NormalizedEvent]) -> list[dict]:
    """Aggregate failed command families without returning command text or stderr."""

    failures = [
        (event.entities.command_family or "unknown", event.entities.exit_code)
        for event in cluster
        if event.family == "command" and event.entities.exit_code not in (None, 0)
    ]
    if not failures:
        return []

    counts = Counter(failures)
    first_seen = {failure: index for index, failure in enumerate(dict.fromkeys(failures))}
    ordered = sorted(counts, key=lambda failure: (-counts[failure], first_seen[failure]))
    return [
        {"command_family": family, "exit_code": exit_code, "count": counts[(family, exit_code)]}
        for family, exit_code in ordered
    ]


def _parent_path(path: str) -> str | None:
    parts = path.replace("\\", "/").rstrip("/").rsplit("/", 1)
    return parts[0] if len(parts) == 2 and parts[0] else None


def _final_segment(path: str) -> str | None:
    normalized = path.replace("\\", "/").rstrip("/")
    if not normalized:
        return None
    segment = normalized.rsplit("/", 1)[-1]
    return segment or None


def _first_most_common(values: list[str]) -> str:
    counts = Counter(values)
    return max(counts, key=counts.get)
