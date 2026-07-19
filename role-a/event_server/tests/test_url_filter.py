from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from fastapi.testclient import TestClient

from event_server.main import create_app
from event_server.url_filter import is_url_blocked, load


class UrlFilterTests(unittest.TestCase):
    def test_exact_subdomain_and_no_tld_rules_are_matched_by_host(self) -> None:
        self.assertTrue(is_url_blocked("https://chase.com/sign-in?account=1", ["chase"]))
        self.assertTrue(is_url_blocked("https://secure.chase.com/sign-in", ["chase.com"]))
        self.assertTrue(is_url_blocked("https://app.internal.company.com", ["internal.company.com"]))
        self.assertFalse(is_url_blocked("https://purchase.com/checkout", ["chase"]))
        self.assertFalse(is_url_blocked("https://notchase.com", ["chase.com"]))

    def test_config_loader_accepts_the_shipped_yaml_shape(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "blocked-domains.yaml"
            path.write_text("blocked_domains:\n  - chase.com # bank\n  - '*.example.com'\n", encoding="utf-8")
            self.assertEqual(load(path)["blocked_domains"], ["chase.com", "example.com"])

    def test_blocked_browser_event_is_stored_without_url_or_context(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            config = directory / "blocked-domains.yaml"
            config.write_text("blocked_domains:\n  - chase\n", encoding="utf-8")
            detailed = directory / "detailed-capture.json"
            detailed.write_text('{"browser": {"enabled": true}}', encoding="utf-8")
            with TestClient(
                create_app(
                    str(directory / "events.db"),
                    detailed_capture_config_path=str(detailed),
                    blocked_domains_config_path=str(config),
                )
            ) as client:
                response = client.post(
                    "/v1/event",
                    json={
                        "id": "00000000-0000-4000-8000-000000000041",
                        "ts": 1783911700,
                        "source": "firefox",
                        "type": "user_action",
                        "payload": {
                            "url": "https://secure.chase.com/sign-in?account=private",
                            "tab_id": 2,
                            "window_id": 1,
                            "action": "click",
                            "target": {"tag": "button", "role": "button", "label": "Transfer money"},
                            "sensitive_page": False,
                            "context": {"kind": "article", "text_excerpt": "private balance"},
                        },
                    },
                )
                self.assertEqual(response.status_code, 201)
                payload = response.json()["event"]["payload"]
                self.assertEqual(payload["url"], "[blocked]")
                self.assertTrue(payload["blocked"])
                self.assertEqual(payload["target"], {"tag": "button", "role": "button"})
                self.assertNotIn("context", payload)
                self.assertEqual(client.get("/v1/config").json(), {"blocked_domains": ["chase"]})
