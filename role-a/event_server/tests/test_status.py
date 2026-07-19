from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from fastapi.testclient import TestClient

from event_server.main import create_app


class StatusApiTests(unittest.TestCase):
    def test_status_reports_counts_by_source(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            with TestClient(create_app(str(Path(temporary) / "events.db"))) as client:
                response = client.post(
                    "/v1/event",
                    json={
                        "id": "00000000-0000-4000-8000-000000000020",
                        "ts": 100,
                        "source": "shell",
                        "type": "command",
                        "payload": {"cmd": "terraform plan", "cwd": "/tmp", "exit_code": 0},
                    },
                )
                self.assertEqual(response.status_code, 201)
                status = client.get("/v1/status")

        self.assertEqual(status.status_code, 200)
        self.assertTrue(status.json()["ok"])
        sources = status.json()["sources"]
        self.assertEqual(sources["shell"]["event_count"], 1)
        self.assertEqual(sources["shell"]["last_event_ts"], 100)
        self.assertFalse(sources["shell"]["healthy"])
        self.assertIn("activity", status.json())
        self.assertEqual(status.json()["services"]["event_server"], True)
