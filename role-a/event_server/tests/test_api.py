from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from fastapi.testclient import TestClient

from event_server.main import create_app


class EventApiTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.client_context = TestClient(create_app(str(Path(self.temp_dir.name) / "events.db")))
        self.client = self.client_context.__enter__()

    def tearDown(self) -> None:
        self.client_context.__exit__(None, None, None)
        self.temp_dir.cleanup()

    def test_event_and_legacy_routes_are_idempotent(self) -> None:
        payload = {
            "id": "00000000-0000-4000-8000-000000000010",
            "ts": 1783911700,
            "source": "firefox",
            "type": "tab_change",
            "payload": {"url": "https://example.com/docs", "title": "Docs", "tab_id": 2},
        }
        first = self.client.post("/v1/event", json=payload)
        duplicate = self.client.post("/event", json=payload)
        listed = self.client.get("/v1/events", params={"since": 1783911700})

        self.assertEqual(first.status_code, 201)
        self.assertTrue(first.json()["inserted"])
        self.assertEqual(duplicate.status_code, 200)
        self.assertFalse(duplicate.json()["inserted"])
        self.assertEqual(listed.status_code, 200)
        self.assertEqual(len(listed.json()), 1)
        self.assertEqual(listed.json()[0]["schema_version"], 1)

    def test_capture_can_be_paused_from_the_local_tray(self) -> None:
        paused = self.client.post("/v1/capture/pause", json={"paused": True})
        payload = {
            "id": "00000000-0000-4000-8000-000000000012",
            "ts": 2,
            "source": "linux",
            "type": "app_focus",
            "payload": {"app": "code", "title": "main.py"},
        }
        ignored = self.client.post("/v1/event", json=payload)
        resumed = self.client.post("/v1/capture/pause", json={"paused": False})

        self.assertEqual(paused.json(), {"ok": True, "paused": True})
        self.assertEqual(ignored.status_code, 204)
        self.assertEqual(resumed.json(), {"ok": True, "paused": False})

    def test_invalid_event_is_rejected(self) -> None:
        response = self.client.post(
            "/v1/event",
            json={
                "id": "00000000-0000-4000-8000-000000000011",
                "ts": 1,
                "source": "vscode",
                "type": "file_open",
                "payload": {},
            },
        )
        self.assertEqual(response.status_code, 422)

    def test_retention_requires_confirmation_after_preview(self) -> None:
        payload = {
            "id": "00000000-0000-4000-8000-000000000013",
            "ts": 1,
            "source": "linux",
            "type": "app_focus",
            "payload": {"app": "code", "title": "main.py"},
        }
        self.assertEqual(self.client.post("/v1/event", json=payload).status_code, 201)

        preview = self.client.get("/v1/retention/preview", params={"metadata_days": 1})
        unconfirmed = self.client.post("/v1/retention/purge", json={"metadata_days": 1})
        purged = self.client.post(
            "/v1/retention/purge", json={"metadata_days": 1, "confirm": True}
        )

        self.assertEqual(preview.status_code, 200)
        self.assertEqual(preview.json()["eligible"]["metadata"], 1)
        self.assertEqual(unconfirmed.status_code, 409)
        self.assertEqual(purged.status_code, 200)
        self.assertEqual(purged.json()["deleted"]["metadata"], 1)
        self.assertEqual(self.client.get("/v1/events").json(), [])
