from __future__ import annotations

import asyncio
import logging
import os
import sys
import tempfile
from datetime import date, datetime, time as datetime_time
from pathlib import Path
from unittest.mock import patch

import httpx
from fastapi.testclient import TestClient

os.environ.setdefault("ROLE_B_DB_PATH", str(Path(tempfile.gettempdir()) / "role-b-backend-flow-import.db"))
sys.path.insert(0, str(Path(__file__).parents[1]))
sys.path.insert(0, str(Path(__file__).parents[2] / "role-a"))

from event_server.main import create_app as create_role_a_app
from event_server.restore import RestoreResult
from intent_engine.api import create_app as create_role_b_app
from intent_engine.labeling import TemplateFallbackLabelProvider
from intent_engine.scheduled_ingest import run_scheduled_ingest
from intent_engine.source import RoleAClient
from intent_engine.store import IntentStore


def run(coroutine):
    return asyncio.run(coroutine)


def _event_timestamp(day: date) -> int:
    local_timezone = datetime.now().astimezone().tzinfo
    return int(datetime.combine(day, datetime_time(hour=12), tzinfo=local_timezone).timestamp())


def test_role_a_capture_to_scheduled_role_b_intent_and_confirmed_restore_only() -> None:
    target_date = date(2026, 7, 14)
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        store = IntentStore(str(root / "intents.db"))
        with (
            patch("event_server.main.configure_jsonl_logger", return_value=logging.getLogger("backend-flow-test")),
            TestClient(create_role_a_app(str(root / "events.db"))) as role_a,
        ):
            ingest = role_a.post("/v1/event", json={
                "id": "00000000-0000-4000-8000-000000000099",
                "ts": _event_timestamp(target_date),
                "source": "vscode",
                "type": "file_save",
                "payload": {"path": "/workspace/intent-os/main.py"},
            })
            assert ingest.status_code == 201

            def handler(request: httpx.Request) -> httpx.Response:
                response = role_a.get(request.url.path, params=dict(request.url.params))
                return httpx.Response(response.status_code, json=response.json(), request=request)

            original_client = httpx.AsyncClient

            def client_factory(**kwargs):
                return original_client(transport=httpx.MockTransport(handler), **kwargs)

            with (
                patch("intent_engine.source.httpx.AsyncClient", side_effect=client_factory),
                patch(
                    "event_server.main.restore",
                    return_value=RestoreResult(ok=True, restored={"files": 0, "urls": 0, "shell": False}, failed=[]),
                ) as restore,
            ):
                role_a_client = RoleAClient("http://role-a")
                scheduled = run(run_scheduled_ingest(
                    store=store,
                    role_a_client=role_a_client,
                    label_provider=TemplateFallbackLabelProvider(),
                    today=target_date,
                    enabled=True,
                ))
                assert target_date.isoformat() in scheduled.processed_dates

                role_b = TestClient(create_role_b_app(store, role_a_client))
                intents = role_b.get("/intents", params={"date": target_date.isoformat()})
                assert intents.status_code == 200
                assert intents.json()

                selected = role_b.post("/resume/select", json={"intent_id": intents.json()[0]["id"]})
                assert selected.status_code == 200
                assert selected.json()["needs_picker"] is False
                preview = selected.json()["selected"]["resume_payload"]
                assert restore.call_count == 0

                confirmed = role_a.post("/v1/restore", json=preview)
                assert confirmed.status_code == 200
                assert restore.call_count == 1
