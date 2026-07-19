from __future__ import annotations

import tempfile
import unittest
import sqlite3
from datetime import datetime
from pathlib import Path

from event_server.models import EventIn
from event_server.storage import EventStore


class EventStoreTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.store = EventStore(Path(self.temp_dir.name) / "events.db")
        self.now = int(datetime.now().astimezone().timestamp())

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    def event(self, event_id: str, ts: int) -> EventIn:
        return EventIn(
            id=event_id,
            ts=ts,
            source="linux",
            type="app_focus",
            payload={"app": "firefox", "title": "Terraform docs"},
        )

    def test_insert_is_append_only_and_idempotent(self) -> None:
        event = self.event("00000000-0000-4000-8000-000000000001", self.now)
        inserted, first = self.store.insert(event, ingested_at=101)
        duplicate, second = self.store.insert(event, ingested_at=102)

        self.assertTrue(inserted)
        self.assertFalse(duplicate)
        self.assertEqual(first.ingested_at, 101)
        self.assertEqual(second.ingested_at, 101)
        self.assertEqual(len(self.store.list_events()), 1)

    def test_insert_many_is_atomic_and_preserves_duplicate_results(self) -> None:
        first = self.event("00000000-0000-4000-8000-000000000008", self.now)
        second = self.event("00000000-0000-4000-8000-000000000009", self.now + 1)

        results = self.store.insert_many([(first, 101), (second, 102), (first, 103)])

        self.assertEqual([inserted for inserted, _event in results], [True, True, False])
        self.assertEqual(results[2][1].ingested_at, 101)
        self.assertEqual(len(self.store.list_events()), 2)

    def test_events_are_sorted_by_timestamp_then_ingestion_time(self) -> None:
        newer = self.event("00000000-0000-4000-8000-000000000002", self.now + 5)
        older = self.event("00000000-0000-4000-8000-000000000003", self.now)
        self.store.insert(newer, ingested_at=20)
        self.store.insert(older, ingested_at=10)

        events = self.store.list_events()
        self.assertEqual([str(event.id) for event in events], [str(older.id), str(newer.id)])

    def test_export_uses_local_calendar_date(self) -> None:
        event = self.event("00000000-0000-4000-8000-000000000004", self.now)
        self.store.insert(event)
        local_date = datetime.fromtimestamp(self.now).astimezone().date().isoformat()

        export = self.store.export_day(local_date, exported_at=999)
        self.assertEqual(export.version, 1)
        self.assertEqual(export.date, local_date)
        self.assertEqual(export.exported_at, 999)
        self.assertEqual([str(item.id) for item in export.events], [str(event.id)])

    def test_rejects_unknown_event_kind(self) -> None:
        with self.assertRaises(ValueError):
            EventIn(
                id="00000000-0000-4000-8000-000000000005",
                ts=self.now,
                source="chrome",
                type="tab_change",
                payload={"url": "https://example.com", "title": "Example"},
            )

    def test_existing_databases_are_migrated_to_schema_version_one(self) -> None:
        legacy_path = Path(self.temp_dir.name) / "legacy.db"
        with sqlite3.connect(legacy_path) as connection:
            connection.execute("CREATE TABLE events (id TEXT PRIMARY KEY, ts INTEGER, source TEXT, type TEXT, payload TEXT, ingested_at INTEGER)")
        migrated = EventStore(legacy_path)
        event = self.event("00000000-0000-4000-8000-000000000006", self.now)

        migrated.insert(event)

        self.assertEqual(migrated.list_events()[0].schema_version, 1)

    def test_source_status_marks_sources_stale_after_configured_threshold(self) -> None:
        event = self.event("00000000-0000-4000-8000-000000000007", 1_000)
        self.store.insert(event)

        stale = self.store.source_status(now=2_801, stale_after_seconds=1_800)
        fresh = self.store.source_status(now=2_800, stale_after_seconds=1_800)

        self.assertFalse(stale["linux"]["healthy"])
        self.assertTrue(fresh["linux"]["healthy"])
        self.assertEqual(
            fresh["firefox"],
            {
                "event_count": 0,
                "last_event_ts": None,
                "last_ingested_at": None,
                "last_ingest_lag_seconds": None,
                "healthy": False,
            },
        )

    def test_retention_preview_and_purge_keep_tiers_separate(self) -> None:
        now = 10_000_000
        expired_metadata = self.event("00000000-0000-4000-8000-000000000021", now - 8 * 86_400)
        expired_detail = EventIn(
            id="00000000-0000-4000-8000-000000000022",
            ts=now - 4 * 86_400,
            source="vscode",
            type="document_change",
            payload={
                "path": "/workspace/main.py",
                "workspace": "/workspace",
                "changes": [
                    {
                        "kind": "insert",
                        "range": {
                            "start": {"line": 0, "character": 0},
                            "end": {"line": 0, "character": 0},
                        },
                        "removed_characters": 0,
                        "text": "safe",
                        "text_length": 4,
                    }
                ],
            },
        )
        fresh_metadata = self.event("00000000-0000-4000-8000-000000000023", now - 86_400)
        self.store.insert_many([(expired_metadata, now), (expired_detail, now), (fresh_metadata, now)])

        preview = self.store.retention_preview(detailed_days=3, metadata_days=7, now=now)
        result = self.store.purge_retention(detailed_days=3, metadata_days=7, now=now)

        self.assertEqual(preview["eligible"], {"detailed": 1, "metadata": 1, "total": 2})
        self.assertEqual(result["deleted"], {"detailed": 1, "metadata": 1, "total": 2})
        self.assertEqual([str(event.id) for event in self.store.list_events()], [str(fresh_metadata.id)])

    def test_metadata_retention_also_limits_detailed_records(self) -> None:
        now = 10_000_000
        detail = EventIn(
            id="00000000-0000-4000-8000-000000000024",
            ts=now - 8 * 86_400,
            source="filesystem",
            type="file_content",
            payload={
                "path": "/workspace/readme.txt",
                "workspace": "/workspace",
                "kind": "text",
                "mime": "text/plain",
                "size_bytes": 4,
                "sha256": "a" * 64,
                "excerpt": "safe",
            },
        )
        self.store.insert(detail, ingested_at=now)

        preview = self.store.retention_preview(metadata_days=7, now=now)

        self.assertEqual(preview["eligible"], {"detailed": 1, "metadata": 0, "total": 1})
