from __future__ import annotations

import json
import os
import sys
import tempfile
import time
from datetime import date, timedelta
from pathlib import Path

os.environ.setdefault("ROLE_B_DB_PATH", str(Path(tempfile.gettempdir()) / "role-b-api-import.db"))
sys.path.insert(0, str(Path(__file__).parents[1]))

from fastapi.testclient import TestClient

from intent_engine.api import create_app
from intent_engine.schemas import EventPayload, Intent, IntentInsights, IntentStats, PipelineResult, ResumePayload, RawEvent
from intent_engine.source import RoleAUnavailableError, load_replay_fixture
from intent_engine.store import IntentStore


class FakeRoleAClient:
    def __init__(self, export, unavailable: bool = False, events=None) -> None:
        self.export = export
        self.unavailable = unavailable
        self.events = events or []
        self.calls = 0

    async def fetch_export(self, date: str):
        self.calls += 1
        if self.unavailable:
            raise RoleAUnavailableError("offline")
        assert date == self.export.date
        return self.export

    async def fetch_events_since(self, since_ts: int):
        return self.events


def test_health_cors_read_routes_and_replay() -> None:
    export = load_replay_fixture(str(Path(__file__).parent / "fixtures" / "demo-day.json"))
    with tempfile.TemporaryDirectory() as directory:
        app = create_app(IntentStore(str(Path(directory) / "intents.db")), FakeRoleAClient(export))
        client = TestClient(app)
        assert client.get("/healthz").json()["ok"] is True
        for origin in ("http://localhost:3000", "http://127.0.0.1:3000", "http://localhost:5000", "http://localhost:5173", "http://localhost:9479"):
            cors = client.options(
                "/healthz", headers={"Origin": origin, "Access-Control-Request-Method": "GET"}
            )
            assert cors.headers["access-control-allow-origin"] == origin
            assert cors.headers["access-control-allow-credentials"] == "true"
        rejected = client.options(
            "/healthz", headers={"Origin": "https://example.com", "Access-Control-Request-Method": "GET"}
        )
        assert "access-control-allow-origin" not in rejected.headers
        external = client.options(
            "/healthz", headers={"Origin": "https://evil.example", "Access-Control-Request-Method": "GET"}
        )
        assert "access-control-allow-origin" not in external.headers
        assert client.get("/intents", params={"date": "2026-02-30"}).status_code == 400
        assert client.get("/intents", params={"date": export.date}).json() == []
        replay = client.post("/pipeline/run-replay", json=export.model_dump(mode="json"))
        assert replay.status_code == 200
        assert replay.json()["intents"]
        assert client.get("/intents", params={"date": export.date}).json()
        assert client.get("/intents/not-found").status_code == 404
        assert client.get("/intents/current").json() is None
        assert client.get("/intents/prediction").json() is None
        search = client.get("/intents/search", params={"q": "Work"})
        assert search.status_code == 200
        assert {"id", "label", "summary", "date", "highlight_snippet"} <= set(search.json()[0])


def test_role_a_run_recompute_and_unavailable_response() -> None:
    export = load_replay_fixture(str(Path(__file__).parent / "fixtures" / "demo-day.json"))
    with tempfile.TemporaryDirectory() as directory:
        role_a = FakeRoleAClient(export)
        client = TestClient(create_app(IntentStore(str(Path(directory) / "intents.db")), role_a))
        first = client.post("/pipeline/run", params={"date": export.date})
        cached = client.post("/pipeline/run", params={"date": export.date})
        recomputed = client.post("/pipeline/recompute", params={"date": export.date})
        assert first.json()["cached"] is False
        assert cached.json()["cached"] is True
        assert recomputed.json()["cached"] is False
        assert role_a.calls == 3

        unavailable = TestClient(create_app(IntentStore(str(Path(directory) / "other.db")), FakeRoleAClient(export, unavailable=True)))
        assert unavailable.post("/pipeline/run", params={"date": export.date}).status_code == 503


def test_search_date_filters_and_invalid_dates() -> None:
    export = load_replay_fixture(str(Path(__file__).parent / "fixtures" / "demo-day.json"))
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        client = TestClient(create_app(store, FakeRoleAClient(export)))
        replay = client.post("/pipeline/run-replay", json=export.model_dump(mode="json"))
        assert replay.status_code == 200
        assert client.get("/intents/search", params={"q": "Work", "date_from": export.date, "date_to": export.date}).status_code == 200
        assert client.get("/intents/search", params={"q": "Work", "date_from": "2026-02-30"}).status_code == 400


def test_prediction_endpoint_is_feature_flagged(monkeypatch) -> None:
    export = load_replay_fixture(str(Path(__file__).parent / "fixtures" / "demo-day.json"))
    fixed_now = int(time.time())
    history_date = (date.today() - timedelta(days=1)).isoformat()
    monkeypatch.setattr("intent_engine.api.time.time", lambda: fixed_now)
    raw_events = [
        RawEvent(id=f"prediction-{index}", ts=fixed_now - 120 + index, source="vscode", type="edit", payload=EventPayload(file_path="/repo/a.py"))
        for index in range(3)
    ]
    child = Intent(
        id="historical-child", parent_id="historical-root", date=history_date, label="Historical Task",
        summary="Historical task.", start_ts=1, end_ts=10, depth=1, prefix=("editor", "edit", ""),
        stats=IntentStats(event_count=3, duration_seconds=9), insights=IntentInsights(), resume_payload=ResumePayload(files=["/repo/a.py"]),
    )
    child_two = child.model_copy(update={"id": "historical-child-two", "label": "Historical Task Two", "end_ts": 20})
    root = Intent(
        id="historical-root", date=history_date, label="Historical Session", summary="Historical session.",
        start_ts=1, end_ts=10, depth=0, stats=IntentStats(event_count=3, duration_seconds=9), insights=IntentInsights(),
        resume_payload=ResumePayload(), children=[child, child_two],
    )
    monkeypatch.setenv("ENABLE_PREDICTION", "true")
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        import asyncio
        asyncio.run(store.save_pipeline_run(history_date, PipelineResult(intents=[root], source_hash="history", pipeline_version="v1")))
        client = TestClient(create_app(store, FakeRoleAClient(export, events=raw_events)))
        response = client.get("/intents/prediction")
    assert response.status_code == 200
    assert response.json()["predicted_label"] == "Historical Task Two"
