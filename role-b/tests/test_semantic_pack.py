import pytest

from intent_engine.schemas import EventEntities, EventSignals, NormalizedEvent
from intent_engine.semantic_pack import (
    MAX_EVENTS_PER_PACKET,
    MAX_SNIPPET_CHARS_PER_EVENT,
    MAX_SNIPPET_CHARS_PER_PACKET,
    build_semantic_candidate_packets,
)


@pytest.fixture(autouse=True)
def clear_semantic_consent(monkeypatch):
    monkeypatch.delenv("ROLE_B_SEMANTIC_CONTENT_CONSENT", raising=False)


def event(event_id, ts, *, family="editor", category="file_edit", raw_payload=None, **entities):
    return NormalizedEvent(
        id=event_id,
        ts=ts,
        ordinal=ts,
        source="vscode" if family == "editor" else "firefox" if family == "browser" else "linux",
        family=family,
        category=category,
        text="private title must not be packed",
        entities=EventEntities(**entities),
        signals=EventSignals(typed_chars=7, save=True, todo_added=False),
        raw={"payload": raw_payload or {}},
    )


def packet_events(packets):
    return [item for packet in packets for item in packet.events]


def test_packets_split_on_time_boundary_and_event_cap():
    events = [event(f"event-{index}", index) for index in range(MAX_EVENTS_PER_PACKET + 1)]
    events.append(event("after-gap", 600))

    packets = build_semantic_candidate_packets(events)

    assert [len(packet.events) for packet in packets] == [MAX_EVENTS_PER_PACKET, 1, 1]
    assert packets[-1].events[0].event_id == "after-gap"


def test_spotify_is_background_only_and_not_task_support():
    work_before = event("work-before", 0)
    spotify = event("spotify", 10, family="focus", category="app_focus", raw_payload={"app": "spotify", "title": "Private song"})
    work_after = event("work-after", 20)

    packets = build_semantic_candidate_packets([work_before, spotify, work_after])

    assert [(packet.events[0].event_id, packet.events[0].deterministic_role) for packet in packets] == [
        ("work-before", "candidate"),
        ("spotify", "background"),
        ("work-after", "candidate"),
    ]
    assert packets[1].events[0].content_snippet is None


def test_whatsapp_is_excluded_from_packets_and_content():
    work = event("work", 0)
    whatsapp = event(
        "whatsapp",
        10,
        family="focus",
        category="app_focus",
        raw_payload={"app": "WhatsApp", "title": "Private message"},
    )

    packets = build_semantic_candidate_packets([work, whatsapp])

    assert [item.event_id for item in packet_events(packets)] == ["work"]
    assert "Private message" not in str(packets)


def test_consent_disabled_keeps_packets_metadata_only():
    editor = event(
        "editor",
        0,
        category="document_change",
        raw_payload={"changes": [{"text": "secret editor content", "redacted": False}]},
    )
    browser = event(
        "browser",
        1,
        family="browser",
        category="user_action",
        raw_payload={"context": {"text_excerpt": "secret browser content"}},
    )

    packed = packet_events(build_semantic_candidate_packets([editor, browser]))

    assert [item.content_snippet for item in packed] == [None, None]
    assert "private title" not in str(packed)
    assert "secret editor content" not in str(packed)
    assert "secret browser content" not in str(packed)


def test_consent_allows_only_capped_editor_and_browser_snippets(monkeypatch):
    monkeypatch.setenv("ROLE_B_SEMANTIC_CONTENT_CONSENT", "true")
    editor = event(
        "editor",
        0,
        category="document_change",
        raw_payload={"changes": [{"text": "a" * 800, "redacted": False}]},
    )
    browser = event(
        "browser",
        1,
        family="browser",
        category="user_action",
        raw_payload={"context": {"text_excerpt": "b" * 800}, "sensitive_page": False, "blocked": False},
    )
    command = event("command", 2, family="command", category="command", raw_payload={"cmd": "private command"})

    packed = packet_events(build_semantic_candidate_packets([editor, browser, command]))

    assert [len(item.content_snippet or "") for item in packed] == [MAX_SNIPPET_CHARS_PER_EVENT] * 2 + [0]
    assert sum(len(item.content_snippet or "") for item in packed) <= MAX_SNIPPET_CHARS_PER_PACKET
    assert "private command" not in str(packed)


def test_private_or_unapproved_content_never_enters_packet(monkeypatch):
    monkeypatch.setenv("ROLE_B_SEMANTIC_CONTENT_CONSENT", "true")
    redacted_editor = event(
        "redacted",
        0,
        category="document_change",
        raw_payload={"changes": [{"text": "[redacted]", "redacted": True}]},
        title="private title",
    )
    sensitive_browser = event(
        "sensitive",
        1,
        family="browser",
        category="user_action",
        raw_payload={"sensitive_page": True, "context": {"text_excerpt": "private browser"}},
        domain="example.com",
    )
    filesystem = event(
        "filesystem",
        2,
        family="file_change",
        category="file_content",
        raw_payload={"excerpt": "private file content", "url": "https://private.example"},
        file_name="secret.txt",
    )

    packed = packet_events(build_semantic_candidate_packets([redacted_editor, sensitive_browser, filesystem]))

    assert all(item.content_snippet is None for item in packed)
    rendered = str(packed)
    for private_value in ("private title", "private browser", "private file content", "private.example"):
        assert private_value not in rendered
