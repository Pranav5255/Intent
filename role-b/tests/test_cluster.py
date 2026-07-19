from __future__ import annotations

import asyncio
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parents[1]))

from intent_engine.cluster import ClusterEngine, cluster_session
from intent_engine.normalize import normalize_events
from intent_engine.schemas import EventEntities, NormalizedEvent
from intent_engine.source import load_replay_fixture


def event(
    event_id: str,
    ts: int,
    *,
    project: str | None = None,
    command: str | None = None,
    file_name: str | None = None,
    domain: str | None = None,
    family: str = "editor",
) -> NormalizedEvent:
    return NormalizedEvent(
        id=event_id,
        ts=ts,
        ordinal=0,
        source="test",
        family=family,
        category="event",
        text="Test event",
        entities=EventEntities(
            project_paths=[project] if project else [],
            command_family=command,
            file_name=file_name,
            domain=domain,
        ),
        raw={},
    )


def run(session: list[NormalizedEvent]) -> list[list[NormalizedEvent]]:
    return asyncio.run(cluster_session(session))


def ids(clusters: list[list[NormalizedEvent]]) -> list[list[str]]:
    return [[event.id for event in cluster] for cluster in clusters]


def test_empty_single_and_five_minute_boundary() -> None:
    one = event("one", 0, project="infra", file_name="iam.tf")
    at_boundary = event("at-boundary", 300, project="infra", file_name="iam.tf")
    beyond_boundary = event("beyond-boundary", 601, project="infra", file_name="iam.tf")

    assert run([]) == []
    assert ids(run([one])) == [["one"]]
    assert ids(run([one, at_boundary, beyond_boundary])) == [["one", "at-boundary", "beyond-boundary"]]


def test_command_and_project_topic_shifts_split() -> None:
    session = [
        event("terraform", 0, project="infra", command="terraform", file_name="iam.tf"),
        event("git", 60, project="infra", command="git", file_name="iam.tf"),
        event("other-project", 120, project="app", command="npm", file_name="package.json"),
    ]

    assert ids(run(session)) == [["terraform"], ["git"], ["other-project"]]


def test_focus_event_stays_with_topic_context() -> None:
    session = [
        event("edit", 0, project="infra", file_name="iam.tf"),
        event("focus", 60, family="focus"),
        event("save", 120, project="infra", file_name="iam.tf"),
    ]

    assert ids(run(session)) == [["edit", "focus", "save"]]


def test_adjacent_similar_clusters_merge_and_engine_keeps_result() -> None:
    session = [
        event("first", 0, project="infra", command="terraform", file_name="iam.tf"),
        event("second", 360, project="infra", command="terraform", file_name="iam.tf"),
    ]
    engine = ClusterEngine()

    clusters = asyncio.run(engine.cluster_session(session))

    assert ids(clusters) == [["first", "second"]]
    assert engine.run_groups == clusters


def test_cluster_cap_preserves_all_events_in_order() -> None:
    commands = ["terraform", "git", "pytest", "python", "npm", "docker"]
    session = [event(command, index * 60, project="infra", command=command) for index, command in enumerate(commands)]

    clusters = run(session)

    assert len(clusters) == 4
    assert [event.id for cluster in clusters for event in cluster] == commands


def test_command_phase_boundary_is_preserved_for_demo_fixture() -> None:
    fixture = Path(__file__).parent / "fixtures" / "demo-day.json"
    export = load_replay_fixture(str(fixture))
    normalized, warnings = normalize_events(export.events)

    clusters = run(normalized)

    assert warnings == []
    assert len(clusters) >= 2
    assert any(event.entities.command_family == "npm" for cluster in clusters for event in cluster)
    assert any(event.category == "document_change" for cluster in clusters for event in cluster)
