from __future__ import annotations

import asyncio
import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import httpx

sys.path.insert(0, str(Path(__file__).parents[1]))

from intent_engine.source import RoleAClient, RoleAUnavailableError, load_replay_fixture


class RoleAClientTests(unittest.TestCase):
    def run_async(self, coroutine):
        return asyncio.run(coroutine)

    def client_for(self, handler, client_options: list[dict] | None = None):
        original_client = httpx.AsyncClient
        transport = httpx.MockTransport(handler)

        def factory(**kwargs):
            if client_options is not None:
                client_options.append(kwargs)
            return original_client(transport=transport, **kwargs)

        return patch(
            "intent_engine.source.httpx.AsyncClient",
            side_effect=factory,
        )

    def test_fetch_export_validates_date_and_parses_day_export(self) -> None:
        requests: list[httpx.Request] = []

        def handler(request: httpx.Request) -> httpx.Response:
            requests.append(request)
            return httpx.Response(200, json={"date": "2026-07-13", "exported_at": 1, "events": []})

        client_options: list[dict] = []
        with self.client_for(handler, client_options):
            export = self.run_async(RoleAClient("http://role-a/").fetch_export("2026-07-13"))

        self.assertEqual(export.date, "2026-07-13")
        self.assertEqual(requests[0].url.path, "/v1/export/day")
        self.assertEqual(requests[0].url.params["date"], "2026-07-13")
        self.assertEqual(client_options[0]["timeout"], 10.0)
        with self.assertRaises(ValueError):
            self.run_async(RoleAClient().fetch_export("2026-7-13"))

    def test_events_health_and_unavailable_errors(self) -> None:
        def handler(request: httpx.Request) -> httpx.Response:
            if request.url.path == "/v1/events":
                return httpx.Response(200, json=[{
                    "id": "event-1", "ts": 3, "source": "linux", "type": "app_focus", "payload": {}, "ingested_at": 4,
                }])
            if request.url.path == "/healthz":
                return httpx.Response(200, json={"ok": True, "version": "0.1", "database": "events.db"})
            return httpx.Response(503)

        with self.client_for(handler):
            client = RoleAClient("http://role-a")
            events = self.run_async(client.fetch_events_since(3))
            health = self.run_async(client.health())

        self.assertEqual(events[0].model_extra, {"ingested_at": 4})
        self.assertTrue(health["ok"])

    def test_unavailable_and_fixture_failures(self) -> None:
        def unavailable(_: httpx.Request) -> httpx.Response:
            return httpx.Response(503)

        with self.client_for(unavailable), self.assertRaises(RoleAUnavailableError):
            self.run_async(RoleAClient("http://role-a").health())

        def connection_error(_: httpx.Request) -> httpx.Response:
            raise httpx.ConnectError("connection refused")

        with self.client_for(connection_error), self.assertRaises(RoleAUnavailableError):
            self.run_async(RoleAClient("http://role-a").health())

        def not_found(_: httpx.Request) -> httpx.Response:
            return httpx.Response(404)

        with self.client_for(not_found), self.assertRaises(httpx.HTTPStatusError):
            self.run_async(RoleAClient("http://role-a").health())

        with self.assertRaises(FileNotFoundError):
            load_replay_fixture("missing-day.json")

        with tempfile.TemporaryDirectory() as directory:
            fixture = Path(directory) / "bad.json"
            fixture.write_text(json.dumps({"date": "2026-07-13"}), encoding="utf-8")
            with self.assertRaises(ValueError):
                load_replay_fixture(str(fixture))

    def test_loads_role_a_fixture(self) -> None:
        fixture = Path(__file__).parents[2] / "role-a" / "fixtures" / "demo-day.json"
        export = load_replay_fixture(str(fixture))
        self.assertEqual(len(export.events), 26)


if __name__ == "__main__":
    unittest.main()
