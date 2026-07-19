from __future__ import annotations

import tempfile
import threading
import time
import unittest
from pathlib import Path

from event_server.ingestion import EventWriter, IngestionUnavailable
from event_server.models import EventIn
from event_server.storage import EventStore


class EventWriterTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.store = EventStore(Path(self.temp_dir.name) / "events.db")

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    @staticmethod
    def event(event_id: str, ts: int) -> EventIn:
        return EventIn(
            id=event_id,
            ts=ts,
            source="linux",
            type="app_focus",
            payload={"app": "code", "title": "main.py"},
        )

    def test_writer_batches_concurrent_durable_submissions(self) -> None:
        writer = EventWriter(self.store, max_batch_size=10, max_batch_wait_ms=75)
        barrier = threading.Barrier(3)
        outcomes = []

        def submit(event: EventIn) -> None:
            barrier.wait()
            outcomes.append(writer.submit(event))

        threads = [
            threading.Thread(target=submit, args=(self.event("00000000-0000-4000-8000-000000000031", 1),)),
            threading.Thread(target=submit, args=(self.event("00000000-0000-4000-8000-000000000032", 2),)),
        ]
        for thread in threads:
            thread.start()
        barrier.wait()
        for thread in threads:
            thread.join()
        snapshot = writer.snapshot()
        writer.close()

        self.assertEqual(len(outcomes), 2)
        self.assertTrue(all(outcome.inserted for outcome in outcomes))
        self.assertEqual(snapshot["accepted"], 2)
        self.assertEqual(snapshot["inserted"], 2)
        self.assertEqual(snapshot["batches"], 1)
        self.assertEqual(len(self.store.list_events()), 2)

    def test_writer_reports_duplicates_and_policy_drops_without_content(self) -> None:
        writer = EventWriter(self.store, max_batch_wait_ms=0)
        event = self.event("00000000-0000-4000-8000-000000000033", 1)

        first = writer.submit(event)
        duplicate = writer.submit(event)
        writer.record_drop("capture_paused")
        snapshot = writer.snapshot()
        writer.close()

        self.assertTrue(first.inserted)
        self.assertFalse(duplicate.inserted)
        self.assertEqual(snapshot["duplicates"], 1)
        self.assertEqual(snapshot["dropped_by_reason"], {"capture_paused": 1})
        self.assertNotIn("payload", snapshot)

    def test_writer_rejects_new_events_when_its_bounded_queue_is_full(self) -> None:
        original_insert_many = self.store.insert_many
        writing_started = threading.Event()
        allow_write = threading.Event()
        outcomes = []

        def blocked_insert_many(records):
            writing_started.set()
            self.assertTrue(allow_write.wait(timeout=2))
            return original_insert_many(records)

        self.store.insert_many = blocked_insert_many  # type: ignore[method-assign]
        writer = EventWriter(self.store, queue_capacity=1, max_batch_wait_ms=0)

        def submit(event: EventIn) -> None:
            outcomes.append(writer.submit(event))

        first = threading.Thread(target=submit, args=(self.event("00000000-0000-4000-8000-000000000034", 1),))
        second = threading.Thread(target=submit, args=(self.event("00000000-0000-4000-8000-000000000035", 2),))
        first.start()
        self.assertTrue(writing_started.wait(timeout=1))
        second.start()
        for _ in range(100):
            if writer.snapshot()["queue_depth"] == 1:
                break
            time.sleep(0.01)
        else:
            self.fail("second submission was not queued")

        try:
            with self.assertRaises(IngestionUnavailable):
                writer.submit(self.event("00000000-0000-4000-8000-000000000036", 3))
        finally:
            allow_write.set()
            first.join(timeout=2)
            second.join(timeout=2)
            writer.close()

        self.assertEqual(len(outcomes), 2)
        self.assertEqual(writer.snapshot()["dropped_by_reason"], {"queue_full": 1})


if __name__ == "__main__":
    unittest.main()
