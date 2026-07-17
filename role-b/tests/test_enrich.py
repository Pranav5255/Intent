from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parents[1]))

from intent_engine.enrich import (
    compute_insight_browser,
    compute_insight_editor,
    compute_insight_shell,
    compute_stats,
    aggregate_stats,
    validate_intent_tree,
    detect_todos,
    derive_project_tag,
)
from intent_engine.schemas import EventEntities, EventSignals, NormalizedEvent
from intent_engine.schemas import Intent, IntentInsights, IntentStats, ResumePayload


def event(
    event_id: str,
    ts: int,
    *,
    source: str = "vscode",
    family: str = "editor",
    project_paths: list[str] | None = None,
    cwd: str | None = None,
    file_path: str | None = None,
    file_name: str | None = None,
    domain: str | None = None,
    command_family: str | None = None,
    exit_code: int | None = None,
    typed_chars: int = 0,
    save: bool = False,
    category: str = "event",
    text: str = "Safe text",
    todo_added: bool = False,
) -> NormalizedEvent:
    return NormalizedEvent(
        id=event_id,
        ts=ts,
        ordinal=0,
        source=source,
        family=family,
        category=category,
        text=text,
        entities=EventEntities(
            project_paths=project_paths or [],
            cwd=cwd,
            file_path=file_path,
            file_name=file_name,
            domain=domain,
            command_family=command_family,
            exit_code=exit_code,
        ),
        signals=EventSignals(typed_chars=typed_chars, save=save, todo_added=todo_added),
        raw={},
    )


def test_project_tag_uses_normalized_paths_and_first_tie() -> None:
    cluster = [
        event("one", 1, project_paths=["C:\\work\\infra"]),
        event("two", 2, cwd="/home/dev/infra"),
        event("three", 3, file_path="/home/dev/app/main.py"),
    ]

    assert derive_project_tag(cluster) == "project:infra"
    assert derive_project_tag([]) is None


def test_stats_handles_empty_and_multiple_events() -> None:
    cluster = [
        event("one", 10, source="vscode"),
        event("two", 25, source="firefox", family="browser"),
        event("three", 40, source="vscode"),
    ]

    assert compute_stats([]).model_dump() == {
        "event_count": 0, "duration_seconds": 0, "sources": {}, "unique_apps": [],
    }
    stats = compute_stats(cluster)
    assert stats.event_count == 3
    assert stats.duration_seconds == 30
    assert stats.sources == {"vscode": 2, "firefox": 1}
    assert stats.unique_apps == ["vscode", "firefox"]


def test_aggregate_stats_merges_children_and_preserves_app_order() -> None:
    def child(child_id: str, events: int, duration: int, sources: dict[str, int], apps: list[str]) -> Intent:
        return Intent(
            id=child_id, date="2026-07-16", label=child_id, summary="Work.", start_ts=1, end_ts=2, depth=1,
            stats=IntentStats(event_count=events, duration_seconds=duration, sources=sources, unique_apps=apps),
            insights=IntentInsights(), resume_payload=ResumePayload(),
        )

    result = aggregate_stats([
        child("one", 3, 10, {"vscode": 2, "firefox": 1}, ["vscode", "firefox"]),
        child("two", 4, 20, {"vscode": 1, "shell": 3}, ["shell", "vscode"]),
    ])
    assert result.event_count == 7
    assert result.duration_seconds == 30
    assert result.sources == {"vscode": 3, "firefox": 1, "shell": 3}
    assert result.unique_apps == ["vscode", "firefox", "shell"]
    assert aggregate_stats([]).model_dump() == {
        "event_count": 0, "duration_seconds": 0, "sources": {}, "unique_apps": [],
    }


def test_validate_intent_tree_accepts_valid_and_empty_roots() -> None:
    child = Intent(
        id="child", parent_id="root", date="2026-07-16", label="Child", summary="Child.",
        start_ts=10, end_ts=20, depth=1, stats=IntentStats(event_count=2, duration_seconds=10),
        insights=IntentInsights(), resume_payload=ResumePayload(),
    )
    root = Intent(
        id="root", date="2026-07-16", label="Root", summary="Root.", start_ts=1, end_ts=30, depth=0,
        stats=IntentStats(event_count=2, duration_seconds=10), insights=IntentInsights(), resume_payload=ResumePayload(),
        children=[child],
    )
    assert validate_intent_tree(root) is True
    empty = root.model_copy(update={"children": [], "stats": IntentStats(event_count=0, duration_seconds=0)})
    assert validate_intent_tree(empty) is True


def test_validate_intent_tree_rejects_invalid_invariants() -> None:
    child = Intent(
        id="child", parent_id="root", date="2026-07-16", label="Child", summary="Child.",
        start_ts=10, end_ts=20, depth=1, stats=IntentStats(event_count=2, duration_seconds=10),
        insights=IntentInsights(), resume_payload=ResumePayload(),
    )
    root = Intent(
        id="root", date="2026-07-16", label="Root", summary="Root.", start_ts=1, end_ts=30, depth=0,
        stats=IntentStats(event_count=2, duration_seconds=10), insights=IntentInsights(), resume_payload=ResumePayload(),
        children=[child],
    )
    invalid_roots = [
        root.model_copy(update={"depth": 1}),
        root.model_copy(update={"children": [child.model_copy(update={"depth": 2})]}),
        root.model_copy(update={"children": [child.model_copy(update={"parent_id": "wrong"})]}),
        root.model_copy(update={"stats": IntentStats(event_count=3, duration_seconds=10)}),
        root.model_copy(update={"start_ts": 11}),
        root.model_copy(update={"end_ts": 19}),
    ]
    for invalid in invalid_roots:
        try:
            validate_intent_tree(invalid)
        except AssertionError:
            pass
        else:
            raise AssertionError("invalid intent tree was accepted")


def test_detect_todos_only_accepts_document_change_editor_events() -> None:
    cluster = [
        event("todo", 10, category="document_change", file_path="/repo/a.py", text="Inserted TODO: add tests", todo_added=True),
        event("fixme", 20, category="document_change", file_path="/repo/b.py", text="Inserted FIXME", todo_added=True),
        event("xxx", 30, category="document_change", file_path="/repo/c.py", text="Inserted XXX", todo_added=True),
        event("default", 40, category="document_change", file_path="/repo/d.py", text="Edited code", todo_added=True),
        event("wrong-category", 50, category="save", file_path="/repo/e.py", text="TODO", todo_added=True),
        event("delete", 60, category="document_change", file_path="/repo/f.py", text="TODO", todo_added=False),
        event("wrong-family", 70, family="browser", category="document_change", file_path="/repo/g.py", text="TODO", todo_added=True),
        event("no-path", 80, category="document_change", text="TODO", todo_added=True),
    ]
    observations = detect_todos(cluster)
    assert [(item.path, item.marker) for item in observations] == [
        ("/repo/a.py", "TODO"), ("/repo/b.py", "FIXME"), ("/repo/c.py", "XXX"), ("/repo/d.py", "TODO")
    ]
    assert all(set(item.model_dump()) == {"path", "observed_ts", "marker"} for item in observations)


def test_editor_insight_aggregates_typed_characters_and_saves() -> None:
    cluster = [
        event("edit-1", 1, file_name="iam.tf", typed_chars=10),
        event("save", 2, file_name="iam.tf", save=True),
        event("edit-2", 3, file_name="main.py", typed_chars=5),
    ]

    assert compute_insight_editor(cluster) == [{"file": "iam.tf", "typed_chars": 15, "saves": 1}]
    assert compute_insight_editor([event("browser", 1, family="browser")]) == []


def test_browser_insights_rank_top_three_without_urls() -> None:
    cluster = [
        event("one", 1, source="firefox", family="browser", domain="docs.example.com"),
        event("two", 2, source="firefox", family="browser", domain="search.example.com"),
        event("three", 3, source="firefox", family="browser", domain="docs.example.com"),
        event("four", 4, source="firefox", family="browser", domain="other.example.com"),
        event("redacted", 5, source="firefox", family="browser"),
    ]

    assert compute_insight_browser(cluster) == [
        {"domain": "docs.example.com", "visits": 2},
        {"domain": "search.example.com", "visits": 1},
        {"domain": "other.example.com", "visits": 1},
    ]


def test_shell_insights_group_failures_without_command_text() -> None:
    cluster = [
        event("failed-1", 1, source="shell", family="command", command_family="terraform", exit_code=1),
        event("success", 2, source="shell", family="command", command_family="terraform", exit_code=0),
        event("failed-2", 3, source="shell", family="command", command_family="terraform", exit_code=1),
        event("failed-3", 4, source="shell", family="command", command_family="git", exit_code=128),
    ]

    insights = compute_insight_shell(cluster)
    assert insights == [
        {"command_family": "terraform", "exit_code": 1, "count": 2},
        {"command_family": "git", "exit_code": 128, "count": 1},
    ]
    assert all("command" not in item for item in insights)
