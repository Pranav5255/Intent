from __future__ import annotations

import asyncio
import os
import sys
import tempfile
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

os.environ.setdefault("ROLE_B_DB_PATH", str(Path(tempfile.gettempdir()) / "role-b-resume-select-import.db"))
sys.path.insert(0, str(Path(__file__).parents[1]))

from fastapi.testclient import TestClient

from intent_engine.api import create_app
from intent_engine.resume_select import select_resume_preview
from intent_engine.schemas import (
    Intent,
    IntentInsights,
    IntentStats,
    PipelineResult,
    ResumePayload,
    ResumeSelectRequest,
    SemanticIntentMetadata,
)
from intent_engine.store import IntentStore


class NoRoleAClient:
    def __init__(self) -> None:
        self.calls = 0

    async def fetch_export(self, _date: str):
        self.calls += 1
        raise AssertionError("resume selection must not fetch Role A")

    async def fetch_events_since(self, _since_ts: int):
        self.calls += 1
        raise AssertionError("resume selection must not fetch Role A")


class InvalidRankClient:
    model = "fake"

    async def respond_json(self, **_kwargs):
        return {"ranked_ids": ["invented"]}


def run(coroutine):
    return asyncio.run(coroutine)


def make_intent(
    intent_id: str,
    *,
    label: str,
    tag: str,
    files: list[str],
    cwd: str | None = None,
    root: str | None = None,
    end_ts: int = 10,
) -> Intent:
    return Intent(
        id=intent_id,
        date="2026-07-13",
        label=label,
        summary=f"Stored work for {label}.",
        start_ts=1,
        end_ts=end_ts,
        depth=0,
        tags=[f"project:{tag}"],
        stats=IntentStats(event_count=2, duration_seconds=9),
        insights=IntentInsights(),
        resume_payload=ResumePayload(files=files, urls=["https://docs.example.test/guide"], shell={"cwd": cwd, "last_cmd": "safe command"} if cwd else {}),
        semantic=SemanticIntentMetadata(workspace_root=root) if root else None,
    )


def seeded_store(directory: str) -> IntentStore:
    store = IntentStore(str(Path(directory) / "intents.db"))
    atlas_one = make_intent("atlas-one", label="Atlas API", tag="atlas", files=["/work/atlas/api.py"], cwd="/work/atlas", root="/work/atlas", end_ts=30)
    atlas_two = make_intent("atlas-two", label="Atlas docs", tag="atlas", files=["/work/atlas/readme.md"], cwd="/work/atlas", root="/work/atlas", end_ts=20)
    other = make_intent("other", label="Other work", tag="other", files=["/work/other/main.py"], cwd="/work/other", root="/work/other")
    run(store.save_pipeline_run("2026-07-13", PipelineResult(intents=[atlas_one, atlas_two, other], source_hash="resume", pipeline_version="v1")))
    return store


def test_select_by_intent_id_returns_only_stored_preview_and_never_calls_role_a() -> None:
    with tempfile.TemporaryDirectory() as directory:
        store = seeded_store(directory)
        role_a = NoRoleAClient()
        client = TestClient(create_app(store, role_a))
        response = client.post("/resume/select", json={"intent_id": "atlas-one", "restore_scope": "same_project"})
    assert response.status_code == 200
    body = response.json()
    assert body["needs_picker"] is False
    assert body["selected"]["intent_id"] == "atlas-one"
    assert body["selected"]["resume_payload"]["files"] == ["/work/atlas/api.py"]
    assert role_a.calls == 0


def test_notification_preview_deep_link_selects_the_scoped_stored_payload() -> None:
    deep_link = "http://127.0.0.1:9479/preview?intent_id=atlas-one&restore_scope=same_project"
    query = parse_qs(urlsplit(deep_link).query)
    with tempfile.TemporaryDirectory() as directory:
        client = TestClient(create_app(seeded_store(directory), NoRoleAClient()))
        response = client.post("/resume/select", json={
            "intent_id": query["intent_id"][0],
            "restore_scope": query["restore_scope"][0],
        })
    assert response.status_code == 200
    assert response.json()["selected"]["intent_id"] == "atlas-one"
    assert response.json()["selected"]["resume_payload"]["files"] == ["/work/atlas/api.py"]


def test_project_query_returns_picker_for_ambiguous_atlas_intents() -> None:
    with tempfile.TemporaryDirectory() as directory:
        client = TestClient(create_app(seeded_store(directory), NoRoleAClient()))
        response = client.post("/resume/select", json={"query": "resume only Project Atlas"})
    assert response.status_code == 200
    body = response.json()
    assert body["needs_picker"] is True
    assert body["selected"] is None
    assert [candidate["intent_id"] for candidate in body["candidates"]] == ["atlas-one", "atlas-two"]


def test_exact_project_tag_selects_and_same_project_filters_local_context() -> None:
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        intent = make_intent(
            "atlas", label="Atlas", tag="atlas", files=["/work/atlas/a.py", "/work/other/b.py"],
            cwd="/work/other", root="/work/atlas",
        )
        run(store.save_pipeline_run("2026-07-13", PipelineResult(intents=[intent], source_hash="scope", pipeline_version="v1")))
        client = TestClient(create_app(store, NoRoleAClient()))
        response = client.post("/resume/select", json={"project_tag": "atlas", "restore_scope": "same_project"})
    assert response.status_code == 200
    payload = response.json()["selected"]["resume_payload"]
    assert payload["files"] == ["/work/atlas/a.py"]
    assert payload["shell"] == {}
    assert payload["urls"] == ["https://docs.example.test/guide"]


def test_missing_workspace_root_removes_local_context() -> None:
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        intent = make_intent("unknown", label="Unknown", tag="unknown", files=[], cwd=None)
        run(store.save_pipeline_run("2026-07-13", PipelineResult(intents=[intent], source_hash="unknown", pipeline_version="v1")))
        client = TestClient(create_app(store, NoRoleAClient()))
        response = client.post("/resume/select", json={"intent_id": "unknown", "restore_scope": "same_project"})
    assert response.status_code == 200
    assert response.json()["selected"]["resume_payload"] == {"files": [], "urls": ["https://docs.example.test/guide"], "shell": {}}


def test_invalid_llm_ranking_falls_back_to_deterministic_order(monkeypatch) -> None:
    monkeypatch.setattr("intent_engine.resume_select.llm_enabled", lambda: True)
    with tempfile.TemporaryDirectory() as directory:
        result = run(select_resume_preview(
            seeded_store(directory), ResumeSelectRequest(query="atlas"), client=InvalidRankClient(),
        ))
    assert result is not None
    assert [candidate.intent_id for candidate in result.candidates] == ["atlas-one", "atlas-two"]
    assert result.needs_picker is True
