import asyncio
import json

import pytest

from intent_engine.llm import LLMError
from intent_engine.schemas import EventEntities, EventSignals, NormalizedEvent
from intent_engine.semantic_cluster import refine_semantic_clusters, refine_semantic_clusters_detailed, semantic_cache_identity


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


@pytest.fixture(autouse=True)
def enable_semantic(monkeypatch):
    monkeypatch.setattr("intent_engine.semantic_cluster.semantic_clustering_enabled", lambda: True)
    monkeypatch.setattr("intent_engine.semantic_cluster.semantic_timeout_ms", lambda: 100)


def run(coroutine):
    return asyncio.run(coroutine)


def event(event_id, ts, *, family="editor", category="file_edit", payload=None, source=None, **entities):
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
        raw={"payload": payload or {}},
    )


def proposal(event_id, role="task", confidence=0.9, links=None):
    return {"event_id": event_id, "role": role, "confidence": confidence, "linked_event_ids": links}


def test_related_editor_and_docs_join_one_task():
    editor = event("editor", 1, project_paths=["/repo/app"], file_name="auth.ts")
    docs = event("docs", 2, family="browser", category="tab_change", domain="developer.mozilla.org")
    client = FakeLLM({"proposals": [proposal("editor", links=["docs"]), proposal("docs", "supporting_context", links=["editor"])]})

    clusters = run(refine_semantic_clusters([editor, docs], client))

    assert [[item.id for item in cluster] for cluster in clusters] == [["editor", "docs"]]
    assert client.calls[0]["schema_name"] == "semantic_cluster_proposals"


def test_background_is_singleton_and_messaging_is_not_sent():
    work = event("work", 1, project_paths=["/repo/app"])
    spotify = event("spotify", 2, family="focus", category="app_focus", payload={"app": "Spotify"})
    whatsapp = event("whatsapp", 3, family="focus", category="app_focus", payload={"app": "WhatsApp", "title": "private"})
    client = FakeLLM({"proposals": [proposal("work", links=["spotify"]), proposal("spotify", "task", links=["work"])]})

    clusters = run(refine_semantic_clusters([work, spotify, whatsapp], client))

    assert [[item.id for item in cluster] for cluster in clusters] == [["work"], ["spotify"]]
    sent_ids = {event["event_id"] for packet in json.loads(client.calls[0]["user"])["packets"] for event in packet["events"]}
    assert sent_ids == {"work", "spotify"}


def test_disjoint_workspaces_cannot_merge_when_model_links_them():
    first = event("first", 1, project_paths=["/repo/one"])
    second = event("second", 2, project_paths=["/repo/two"])
    client = FakeLLM({"proposals": [proposal("first", links=["second"]), proposal("second", links=["first"])]})

    clusters = run(refine_semantic_clusters([first, second], client))

    assert [[item.id for item in cluster] for cluster in clusters] == [["first"], ["second"]]


def test_low_confidence_links_leave_events_separate():
    first = event("first", 1)
    second = event("second", 2, family="browser", category="tab_change")
    client = FakeLLM({"proposals": [proposal("first", confidence=0.69, links=["second"]), proposal("second", links=["first"])]})

    clusters = run(refine_semantic_clusters([first, second], client))

    assert [[item.id for item in cluster] for cluster in clusters] == [["first"], ["second"]]


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


@pytest.mark.parametrize("provider", ["openai", "gemini"])
def test_factory_paths_and_cache_identity(monkeypatch, provider):
    client = FakeLLM({"proposals": [proposal("known")]}, model=f"{provider}-model")
    monkeypatch.setenv("LLM_PROVIDER", provider)
    monkeypatch.setattr("intent_engine.semantic_cluster.create_llm_client", lambda: client)

    clusters = run(refine_semantic_clusters([event("known", 1)]))

    assert [[item.id for item in cluster] for cluster in clusters] == [["known"]]
    assert semantic_cache_identity(client) == f"semantic:{provider}:{provider}-model:content-policy-1:cluster-policy-1"
