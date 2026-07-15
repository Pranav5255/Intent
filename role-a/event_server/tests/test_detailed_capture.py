from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from fastapi.testclient import TestClient

from event_server.detailed_capture import DEFAULT_CONFIG, save
from event_server.main import create_app


class DetailedCaptureApiTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.workspace = self.root / "workspace"
        self.workspace.mkdir()
        self.config_path = self.root / "detailed-capture.json"
        self.client_context = TestClient(
            create_app(str(self.root / "events.db"), detailed_capture_config_path=str(self.config_path))
        )
        self.client = self.client_context.__enter__()
        self.workspaces = patch("event_server.detailed_capture.approved_workspaces", return_value=[str(self.workspace)])
        self.workspaces.start()

    def tearDown(self) -> None:
        self.workspaces.stop()
        self.client_context.__exit__(None, None, None)
        self.temporary.cleanup()

    def enable(self, editor: bool = False, browser: bool = False) -> None:
        config = DEFAULT_CONFIG.copy()
        config["editor"] = {**DEFAULT_CONFIG["editor"], "enabled": editor}
        config["browser"] = {"enabled": browser}
        save(config, self.config_path)

    def document_event(self, text: str = "abc") -> dict[str, object]:
        return {
            "id": "00000000-0000-4000-8000-000000000030",
            "ts": 1783911700,
            "source": "vscode",
            "type": "document_change",
            "payload": {
                "path": str(self.workspace / "main.py"),
                "workspace": str(self.workspace),
                "language": "python",
                "changes": [
                    {
                        "kind": "insert",
                        "range": {
                            "start": {"line": 1, "character": 0},
                            "end": {"line": 1, "character": 0},
                        },
                        "removed_characters": 0,
                        "text": text,
                        "text_length": len(text),
                    }
                ],
            },
        }

    def test_disabled_detailed_capture_is_not_persisted(self) -> None:
        response = self.client.post("/v1/event", json=self.document_event())
        self.assertEqual(response.status_code, 204)
        self.assertEqual(self.client.get("/v1/events").json(), [])

    def test_approved_document_change_is_redacted_and_exported(self) -> None:
        self.enable(editor=True)
        response = self.client.post("/v1/event", json=self.document_event("API_KEY=super-secret"))
        self.assertEqual(response.status_code, 201)
        event = response.json()["event"]
        self.assertEqual(event["payload"]["changes"][0]["text"], "[redacted]")
        self.assertTrue(event["payload"]["changes"][0]["redacted"])
        self.assertEqual(self.client.get("/v1/status").json()["detailed_capture"]["event_counts"], {"vscode/document_change": 1})
        exported = self.client.get("/v1/export/day", params={"date": "2026-07-13"}).json()
        self.assertEqual(exported["events"][0]["type"], "document_change")

    def test_browser_actions_require_consent_and_sensitive_targets_are_strict(self) -> None:
        event = {
            "id": "00000000-0000-4000-8000-000000000031",
            "ts": 1783911700,
            "source": "firefox",
            "type": "user_action",
            "payload": {
                "url": "https://example.com/settings?token=secret",
                "tab_id": 1,
                "window_id": 2,
                "action": "click",
                "target": {"tag": "button", "role": "button", "label": "Save"},
                "sensitive_page": False,
            },
        }
        self.assertEqual(self.client.post("/v1/event", json=event).status_code, 204)
        self.enable(browser=True)
        response = self.client.post("/v1/event", json=event)
        self.assertEqual(response.status_code, 201)
        self.assertEqual(response.json()["event"]["payload"]["url"], "https://example.com/settings")

        event["id"] = "00000000-0000-4000-8000-000000000032"
        event["payload"]["sensitive_page"] = True
        self.assertEqual(self.client.post("/v1/event", json=event).status_code, 422)

    def test_purge_deletes_only_detailed_events(self) -> None:
        self.enable(editor=True)
        self.assertEqual(self.client.post("/v1/event", json=self.document_event()).status_code, 201)
        response = self.client.post("/v1/detailed-capture/purge")
        self.assertEqual(response.status_code, 200)
        self.assertEqual(response.json(), {"ok": True, "deleted": 1})
