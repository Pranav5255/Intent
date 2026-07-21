import asyncio
import json

import pytest

from intent_engine.llm import LLMError
from intent_engine.schemas import EventEntities, EventSignals, NormalizedEvent, SemanticProposalResponse
from intent_engine.semantic_cluster import (
    MAX_FULL_CAPTURE_PROMPT_CHARS,
    MAX_GROQ_FULL_CAPTURE_PROMPT_CHARS,
    MAX_SEMANTIC_PROMPT_CHARS,
    _normalize_single_packet_response,
    refine_semantic_clusters,
    refine_semantic_clusters_detailed,
    semantic_cache_identity,
)


class FakeLLM:
    def __init__(self, response=None, error=None, delay=0, model="test-model"):
        self.response = response
        self.error = error
        self.delay = delay
        self.model = model
        self.calls = []

    async def respond_json(self, **kwargs):
        self.calls.append(kwargs)
        if self.delay:
            await asyncio.sleep(self.delay)
        if self.error:
            raise self.error
        return self.response


class BatchedLinkingFakeLLM:
    """Links adjacent request-local events to exercise overlap merging."""

    def __init__(self):
        self.model = "batched-test-model"
        self.calls = []

    async def respond_json(self, **kwargs):
        self.calls.append(kwargs)
        payload = json.loads(kwargs["user"])
        event_ids = [packet["packet_id"] for packet in payload["packets"]]
        return {
            "proposals": [
                proposal(event_id, links=[event_ids[index - 1]] if index else [])
                for index, event_id in enumerate(event_ids)
            ]
        }


class FullCaptureLinkingFakeLLM:
    """Models raw packet decisions and the no-raw cross-batch linker."""

    def __init__(self):
        self.model = "full-capture-test-model"
        self.calls = []

    async def respond_json(self, **kwargs):
        self.calls.append(kwargs)
        payload = json.loads(kwargs["user"])
        if "record_format" in payload:
            packet_ids = [packet[0] for packet in payload["packets"]]
        else:
            packet_ids = [packet[0] for packet in payload["packets"]]
        return {
            "proposals": [
                proposal(
                    packet_id,
                    topic="OAuth PKCE authentication",
                    links=[packet_ids[index - 1]] if "record_format" not in payload and index else [],
                )
                for index, packet_id in enumerate(packet_ids)
            ]
        }


class RateLimitedFakeLLM:
    def __init__(self):
        self.model = "rate-limit-test-model"
        self.calls = 0

    async def respond_json(self, **kwargs):
        self.calls += 1
        if self.calls == 1:
            raise LLMError("rate limited", status_code=429, retry_after_seconds=0)
        return {"proposals": [proposal("p0")]}


class JsonValidationRetryFakeLLM:
    def __init__(self):
        self.model = "json-validation-test-model"
        self.calls = 0

    async def respond_json(self, **kwargs):
        self.calls += 1
        if self.calls == 1:
            raise LLMError("invalid JSON", status_code=400, error_code="json_validate_failed")
        return {"proposals": [proposal("p0")]}


@pytest.fixture(autouse=True)
def enable_semantic(monkeypatch):
    monkeypatch.setattr("intent_engine.semantic_cluster.semantic_clustering_enabled", lambda: True)
    monkeypatch.setattr("intent_engine.semantic_cluster.semantic_timeout_ms", lambda: 100)
    monkeypatch.delenv("ROLE_B_SEMANTIC_CONTENT_CONSENT", raising=False)
    monkeypatch.delenv("ROLE_B_SEMANTIC_FULL_CAPTURE_CONSENT", raising=False)
    monkeypatch.delenv("LLM_PROVIDER", raising=False)


def run(coroutine):
    return asyncio.run(coroutine)


def event(event_id, ts, *, family="editor", category="file_edit", payload=None, raw=None, source=None, **entities):
    if source is None:
        source = "vscode" if family == "editor" else "firefox" if family == "browser" else "linux"
    return NormalizedEvent(
        id=event_id,
        ts=ts,
        ordinal=ts,
        source=source,
        family=family,
        category=category,
        text="not included in semantic packets",
        entities=EventEntities(**entities),
        signals=EventSignals(),
        raw=raw if raw is not None else {"payload": payload or {}},
    )


def proposal(event_id, role="task", confidence=0.9, links=None, topic="unknown"):
    return {"event_id": event_id, "role": role, "confidence": confidence, "topic": topic, "linked_event_ids": links or []}


def test_related_editor_and_docs_join_one_task():
    editor = event("editor", 1, project_paths=["/repo/app"], file_name="auth.ts")
    docs = event("docs", 2, family="browser", category="tab_change", domain="developer.mozilla.org")
    client = FakeLLM({"proposals": [proposal("p0")]})

    clusters = run(refine_semantic_clusters([editor, docs], client))

    assert [[item.id for item in cluster] for cluster in clusters] == [["editor", "docs"]]
    assert client.calls[0]["schema_name"] == "semantic_cluster_proposals"


def test_background_is_singleton_and_messaging_is_not_sent():
    work = event("work", 1, project_paths=["/repo/app"])
    spotify = event("spotify", 2, family="focus", category="app_focus", payload={"app": "Spotify"})
    whatsapp = event("whatsapp", 3, family="focus", category="app_focus", payload={"app": "WhatsApp", "title": "private"})
    client = FakeLLM({"proposals": [proposal("p0", links=["p1"]), proposal("p1", "task", links=["p0"])]})

    clusters = run(refine_semantic_clusters([work, spotify, whatsapp], client))

    assert [[item.id for item in cluster] for cluster in clusters] == [["work"], ["spotify"], ["whatsapp"]]
    sent_ids = {packet["packet_id"] for packet in json.loads(client.calls[0]["user"])["packets"]}
    assert sent_ids == {"p0", "p1"}


def test_disjoint_workspaces_cannot_merge_when_model_links_them():
    first = event("first", 1, project_paths=["/repo/one"])
    second = event("second", 2, project_paths=["/repo/two"])
    client = FakeLLM({"proposals": [proposal("p0")]})

    clusters = run(refine_semantic_clusters([first, second], client))

    assert [[item.id for item in cluster] for cluster in clusters] == [["first"], ["second"]]


def test_low_confidence_links_leave_events_separate():
    first = event("first", 1)
    second = event("second", 302, family="browser", category="tab_change")
    client = FakeLLM({"proposals": [proposal("p0", confidence=0.69, links=["p1"]), proposal("p1", links=["p0"])]})

    clusters = run(refine_semantic_clusters([first, second], client))

    assert [[item.id for item in cluster] for cluster in clusters] == [["first"], ["second"]]


def test_adjacent_nonwork_browser_packets_remain_one_browsing_phase():
    events = [
        event(f"browser-{index}", index, family="browser", category="tab_change")
        for index in range(13)
    ]
    client = FakeLLM({"proposals": [proposal("p0", "background"), proposal("p1", "unrelated")]})

    clusters = run(refine_semantic_clusters(events, client))

    assert [[item.id for item in cluster] for cluster in clusters] == [[event.id for event in events]]


def test_adjacent_editor_packets_in_one_workspace_remain_one_work_chain():
    events = [
        event(
            f"editor-{index}",
            index,
            category="document_change",
            project_paths=["/repo/app"],
            file_name="main.py",
        )
        for index in range(13)
    ]
    client = FakeLLM({"proposals": [proposal("p0"), proposal("p1", "supporting_context")]})

    clusters = run(refine_semantic_clusters(events, client))

    assert [[item.id for item in cluster] for cluster in clusters] == [[event.id for event in events]]


def test_large_sessions_are_batched_without_truncation_and_keep_overlap_links(monkeypatch):
    monkeypatch.setenv("ROLE_B_SEMANTIC_CONTENT_CONSENT", "true")
    events = [
        event(
            f"event-{index}",
            index * 301,
            category="document_change",
            payload={"changes": [{"text": "x" * 500, "redacted": False}]},
        )
        for index in range(20)
    ]
    client = BatchedLinkingFakeLLM()

    clusters = run(refine_semantic_clusters(events, client))

    assert len(client.calls) > 1
    assert all(len(call["user"]) <= MAX_SEMANTIC_PROMPT_CHARS for call in client.calls)
    requested_ids = {
        packet["packet_id"]
        for call in client.calls
        for packet in json.loads(call["user"])["packets"]
    }
    assert requested_ids == {f"p{index}" for index in range(len(events))}
    assert [[item.id for item in cluster] for cluster in clusters] == [[event.id for event in events]]


def _decode_full_capture_prompt(prompt: str) -> list[dict]:
    """Test-only decoder for the lossless Role A event codec."""

    payload = json.loads(prompt)
    dictionary = payload["dictionary"]
    aliases = payload["key_legend"]

    def decode(value):
        if isinstance(value, str):
            if value.startswith("@@"):
                return value[1:]
            if value.startswith("@") and value[1:].isdigit():
                return dictionary[int(value[1:])]
            return value
        if isinstance(value, list):
            return [decode(item) for item in value]
        if isinstance(value, dict):
            decoded = {}
            for key, item in value.items():
                raw_key = aliases.get(key, key[1:] if key.startswith("!") else key)
                decoded[raw_key] = decode(item)
            return decoded
        return value

    events = []
    for event_id, ts_delta, type_index, raw_payload, envelope in payload["events"]:
        source, event_type = payload["type_legend"][type_index]
        decoded = decode(envelope)
        decoded.update({
            "id": decode(event_id),
            "ts": payload["base_ts"] + ts_delta,
            "source": source,
            "type": event_type,
            "payload": decode(raw_payload),
        })
        events.append(decoded)
    return events


def test_full_capture_prompt_losslessly_includes_every_role_a_event_field(monkeypatch):
    monkeypatch.setenv("ROLE_B_SEMANTIC_CONTENT_CONSENT", "true")
    monkeypatch.setenv("ROLE_B_SEMANTIC_FULL_CAPTURE_CONSENT", "true")
    original_events = [
        {
            "id": "00000000-0000-4000-8000-000000000001",
            "ts": 100,
            "source": "firefox",
            "type": "user_action",
            "schema_version": 1,
            "ingested_at": 110,
            "payload": {
                "url": "https://docs.example.test/authentication",
                "title": "Authentication design notes",
                "context": {"text_excerpt": "OAuth PKCE implementation details"},
            },
        },
        {
            "id": "00000000-0000-4000-8000-000000000002",
            "ts": 101,
            "source": "vscode",
            "type": "document_change",
            "schema_version": 1,
            "ingested_at": 111,
            "payload": {
                "path": "/repo/auth.ts",
                "workspace": "/repo",
                "changes": [{"text": "implement OAuth PKCE", "redacted": False}],
            },
        },
    ]
    session = [
        event("normalized-one", 100, family="browser", category="user_action", raw=original_events[0]),
        event("normalized-two", 101, category="document_change", raw=original_events[1]),
    ]
    client = FakeLLM({"proposals": [proposal("p0", topic="OAuth PKCE authentication")]})

    clusters = run(refine_semantic_clusters(session, client))

    assert [[item.id for item in cluster] for cluster in clusters] == [["normalized-one", "normalized-two"]]
    assert client.calls[0]["max_input_chars"] == MAX_FULL_CAPTURE_PROMPT_CHARS
    assert len(client.calls[0]["user"]) <= MAX_FULL_CAPTURE_PROMPT_CHARS
    assert json.loads(client.calls[0]["user"])["packet_format"] == [
        "packet_id", "deterministic_role", "event_offset", "event_count"
    ]
    assert json.loads(client.calls[0]["user"])["output_packet_ids"] == ["p0"]
    assert _decode_full_capture_prompt(client.calls[0]["user"]) == original_events


def test_direct_proposal_response_is_normalized_only_for_a_single_packet():
    direct = proposal("p0")

    assert _normalize_single_packet_response(direct, 1) == {"proposals": [direct]}
    assert _normalize_single_packet_response(direct, 2) == direct


def test_full_capture_sends_each_raw_event_once_and_links_topics_across_groq_batches(monkeypatch):
    monkeypatch.setenv("ROLE_B_SEMANTIC_CONTENT_CONSENT", "true")
    monkeypatch.setenv("ROLE_B_SEMANTIC_FULL_CAPTURE_CONSENT", "true")
    monkeypatch.setenv("LLM_PROVIDER", "groq")
    monkeypatch.setattr("intent_engine.semantic_cluster.MAX_GROQ_FULL_CAPTURE_PROMPT_CHARS", 3_000)
    session = [
        event(
            f"event-{index}",
            index,
            category="document_change",
            payload={"changes": [{"text": f"OAuth PKCE change {index}: " + ("x" * 120)}]},
        )
        for index in range(25)
    ]
    client = FullCaptureLinkingFakeLLM()

    clusters = run(refine_semantic_clusters(session, client))

    raw_calls = [json.loads(call["user"]) for call in client.calls if "record_format" in json.loads(call["user"])]
    link_calls = [json.loads(call["user"]) for call in client.calls if "record_format" not in json.loads(call["user"])]
    forwarded_ids = [record[0] for payload in raw_calls for record in payload["events"]]
    assert len(raw_calls) > 1
    assert len(forwarded_ids) == len(session)
    assert set(forwarded_ids) == {event.id for event in session}
    assert all(len(json.dumps(payload, separators=(",", ":"))) <= 3_000 for payload in raw_calls)
    assert len(link_calls) == 1
    assert "events" not in link_calls[0]
    assert all(call["prefer_json_object"] is True for call in client.calls)
    assert [[item.id for item in cluster] for cluster in clusters] == [[event.id for event in session]]


def test_semantic_response_schema_is_strict_for_response_format_providers():
    schema = SemanticProposalResponse.model_json_schema()

    assert schema["additionalProperties"] is False
    assert schema["$defs"]["SemanticEventProposal"]["additionalProperties"] is False
    assert schema["required"] == ["proposals"]
    assert schema["$defs"]["SemanticEventProposal"]["required"] == [
        "event_id", "role", "confidence", "topic", "linked_event_ids"
    ]


@pytest.mark.parametrize(
    "response",
    [
        {"proposals": [proposal("unknown")]},
        {"proposals": [proposal("known", links=["unknown"])]},
        {"proposals": []},
        {"invalid": True},
    ],
)
def test_invalid_or_malformed_proposals_fall_back(response):
    client = FakeLLM(response)
    assert run(refine_semantic_clusters([event("known", 1)], client)) is None


def test_provider_error_and_timeout_fall_back(monkeypatch):
    assert run(refine_semantic_clusters([event("known", 1)], FakeLLM(error=LLMError("failed")))) is None
    monkeypatch.setattr("intent_engine.semantic_cluster.semantic_timeout_ms", lambda: 1)
    assert run(refine_semantic_clusters([event("known", 1)], FakeLLM(response={}, delay=0.1))) is None


def test_rate_limited_semantic_request_retries_without_rebuilding_the_packet():
    client = RateLimitedFakeLLM()

    clusters = run(refine_semantic_clusters([event("known", 1)], client))

    assert [[item.id for item in cluster] for cluster in clusters] == [["known"]]
    assert client.calls == 2


def test_json_validation_failure_retries_without_rebuilding_the_packet():
    client = JsonValidationRetryFakeLLM()

    clusters = run(refine_semantic_clusters([event("known", 1)], client))

    assert [[item.id for item in cluster] for cluster in clusters] == [["known"]]
    assert client.calls == 2


def test_detailed_fallback_reasons_are_normalized():
    timeout = run(refine_semantic_clusters_detailed([event("known", 1)], FakeLLM(error=LLMError("LLM request timed out"))))
    malformed = run(refine_semantic_clusters_detailed([event("known", 1)], FakeLLM({"proposals": "invalid"})))

    assert timeout.fallback_reason == "timeout"
    assert malformed.fallback_reason == "invalid_response"


def test_disabled_semantic_clustering_does_not_call_client(monkeypatch):
    monkeypatch.setattr("intent_engine.semantic_cluster.semantic_clustering_enabled", lambda: False)
    client = FakeLLM({"proposals": []})
    assert run(refine_semantic_clusters([event("known", 1)], client)) is None
    assert client.calls == []


@pytest.mark.parametrize("provider", ["openai", "gemini", "groq"])
def test_factory_paths_and_cache_identity(monkeypatch, provider):
    client = FakeLLM({"proposals": [proposal("p0")]}, model=f"{provider}-model")
    monkeypatch.setenv("LLM_PROVIDER", provider)
    monkeypatch.setattr("intent_engine.semantic_cluster.create_semantic_llm_client", lambda: client)

    clusters = run(refine_semantic_clusters([event("known", 1)]))

    assert [[item.id for item in cluster] for cluster in clusters] == [["known"]]
    assert semantic_cache_identity(client) == f"semantic:{provider}:{provider}-model:content-policy-3:cluster-policy-17"
