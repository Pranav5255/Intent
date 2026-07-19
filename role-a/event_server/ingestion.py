"""Bounded, durable batching for approved Role A events.

The HTTP handler performs validation, consent checks, and redaction before an
event reaches this writer.  ``submit`` waits for durable SQLite persistence, so
collectors retain the existing at-least-once/idempotent contract while bursty
sources share one transaction.
"""

from __future__ import annotations

import queue
import threading
import time
from collections import Counter
from dataclasses import dataclass
from typing import Any, Callable

from .models import EventIn, EventOut
from .storage import EventStore


class IngestionUnavailable(RuntimeError):
    """The local writer cannot safely accept a new event."""


@dataclass
class IngestionOutcome:
    """Durable result returned to the HTTP handler for one submitted event."""

    inserted: bool
    event: EventOut


@dataclass
class _PendingEvent:
    event: EventIn
    ingested_at: int
    enqueued_at: float
    completed: threading.Event
    outcome: IngestionOutcome | None = None
    error: Exception | None = None


class IngestionMetrics:
    """Small, thread-safe operational snapshot with no activity content."""

    def __init__(self, queue_capacity: int) -> None:
        self._lock = threading.Lock()
        self._queue_capacity = queue_capacity
        self._started_at = int(time.time())
        self._accepted = 0
        self._inserted = 0
        self._duplicates = 0
        self._batches = 0
        self._write_failures = 0
        self._timeouts = 0
        self._dropped = Counter()
        self._queue_high_watermark = 0
        self._last_write_at: int | None = None
        self._last_error: str | None = None
        self._total_queue_wait_ms = 0
        self._total_write_ms = 0

    def record_enqueue(self, queue_depth: int) -> None:
        with self._lock:
            self._queue_high_watermark = max(self._queue_high_watermark, queue_depth)

    def record_drop(self, reason: str) -> None:
        with self._lock:
            self._dropped[reason] += 1

    def record_timeout(self) -> None:
        with self._lock:
            self._timeouts += 1

    def record_batch(
        self,
        pending: list[_PendingEvent],
        outcomes: list[tuple[bool, EventOut]],
        write_duration_ms: int,
    ) -> None:
        queue_wait_ms = sum(int((time.monotonic() - item.enqueued_at) * 1000) for item in pending)
        with self._lock:
            self._accepted += len(outcomes)
            self._inserted += sum(1 for inserted, _event in outcomes if inserted)
            self._duplicates += sum(1 for inserted, _event in outcomes if not inserted)
            self._batches += 1
            self._last_write_at = int(time.time())
            self._last_error = None
            self._total_queue_wait_ms += queue_wait_ms
            self._total_write_ms += write_duration_ms

    def record_write_failure(self, error: Exception) -> None:
        with self._lock:
            self._write_failures += 1
            self._last_error = type(error).__name__

    def snapshot(self, queue_depth: int, accepting: bool) -> dict[str, Any]:
        with self._lock:
            completed = self._accepted
            return {
                "state": "degraded" if self._last_error else ("ready" if accepting else "stopping"),
                "started_at": self._started_at,
                "queue_depth": queue_depth,
                "queue_capacity": self._queue_capacity,
                "queue_high_watermark": self._queue_high_watermark,
                "accepted": self._accepted,
                "inserted": self._inserted,
                "duplicates": self._duplicates,
                "dropped": sum(self._dropped.values()),
                "dropped_by_reason": dict(sorted(self._dropped.items())),
                "batches": self._batches,
                "write_failures": self._write_failures,
                "submit_timeouts": self._timeouts,
                "last_write_at": self._last_write_at,
                "last_error": self._last_error,
                "average_queue_wait_ms": round(self._total_queue_wait_ms / completed, 2) if completed else 0.0,
                "average_write_ms": round(self._total_write_ms / self._batches, 2) if self._batches else 0.0,
            }


class EventWriter:
    """Serialize SQLite writes in bounded batches without weakening durability."""

    _STOP = object()

    def __init__(
        self,
        store: EventStore,
        *,
        queue_capacity: int = 1_024,
        max_batch_size: int = 100,
        max_batch_wait_ms: int = 25,
        submit_timeout_seconds: float = 5.0,
        after_batch: Callable[[], None] | None = None,
    ) -> None:
        if queue_capacity <= 0:
            raise ValueError("queue_capacity must be positive")
        if max_batch_size <= 0:
            raise ValueError("max_batch_size must be positive")
        if max_batch_wait_ms < 0:
            raise ValueError("max_batch_wait_ms must not be negative")
        if submit_timeout_seconds <= 0:
            raise ValueError("submit_timeout_seconds must be positive")

        self._store = store
        self._queue: queue.Queue[_PendingEvent | object] = queue.Queue(maxsize=queue_capacity)
        self._max_batch_size = max_batch_size
        self._max_batch_wait_seconds = max_batch_wait_ms / 1000
        self._submit_timeout_seconds = submit_timeout_seconds
        self._after_batch = after_batch
        self._metrics = IngestionMetrics(queue_capacity)
        self._state_lock = threading.Lock()
        self._accepting = True
        self._thread = threading.Thread(target=self._run, name="intent-os-event-writer", daemon=True)
        self._thread.start()

    def submit(self, event: EventIn) -> IngestionOutcome:
        """Persist one approved event, waiting until its batch commits or fails."""

        pending = _PendingEvent(
            event=event,
            ingested_at=int(time.time()),
            enqueued_at=time.monotonic(),
            completed=threading.Event(),
        )
        with self._state_lock:
            if not self._accepting:
                self._metrics.record_drop("writer_stopping")
                raise IngestionUnavailable("event writer is stopping")
            try:
                self._queue.put_nowait(pending)
            except queue.Full as exc:
                self._metrics.record_drop("queue_full")
                raise IngestionUnavailable("event writer queue is full") from exc
            self._metrics.record_enqueue(self._queue.qsize())

        if not pending.completed.wait(self._submit_timeout_seconds):
            self._metrics.record_timeout()
            raise IngestionUnavailable("timed out waiting for durable event persistence")
        if pending.error is not None:
            raise IngestionUnavailable("event persistence failed") from pending.error
        if pending.outcome is None:  # Defensive: completion must always carry a terminal outcome.
            raise IngestionUnavailable("event writer returned no persistence outcome")
        return pending.outcome

    def record_drop(self, reason: str) -> None:
        """Expose policy/paused drops alongside writer backpressure drops."""

        self._metrics.record_drop(reason)

    def snapshot(self) -> dict[str, Any]:
        with self._state_lock:
            accepting = self._accepting
        return self._metrics.snapshot(self._queue.qsize(), accepting)

    def close(self, timeout_seconds: float = 5.0) -> None:
        """Stop accepting events and drain all previously accepted work."""

        with self._state_lock:
            if not self._accepting:
                return
            self._accepting = False
            try:
                self._queue.put(self._STOP, timeout=timeout_seconds)
            except queue.Full:
                # The worker will make space shortly; a blocking sentinel is safer
                # than abandoning accepted events during service shutdown.
                self._queue.put(self._STOP)
        self._thread.join(timeout_seconds)

    def _run(self) -> None:
        stopping = False
        while not stopping:
            item = self._queue.get()
            if item is self._STOP:
                self._queue.task_done()
                break
            batch = [item]
            deadline = time.monotonic() + self._max_batch_wait_seconds
            while len(batch) < self._max_batch_size:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    break
                try:
                    next_item = self._queue.get(timeout=remaining)
                except queue.Empty:
                    break
                if next_item is self._STOP:
                    stopping = True
                    self._queue.task_done()
                    break
                batch.append(next_item)

            started_at = time.monotonic()
            try:
                outcomes = self._store.insert_many([(item.event, item.ingested_at) for item in batch])
            except Exception as exc:
                self._metrics.record_write_failure(exc)
                for pending in batch:
                    pending.error = exc
                    pending.completed.set()
                    self._queue.task_done()
                continue

            self._metrics.record_batch(batch, outcomes, int((time.monotonic() - started_at) * 1000))
            for pending, (inserted, event) in zip(batch, outcomes, strict=True):
                pending.outcome = IngestionOutcome(inserted=inserted, event=event)
                pending.completed.set()
                self._queue.task_done()

            if self._after_batch is not None:
                try:
                    self._after_batch()
                except Exception:
                    # Retention/maintenance is best effort.  Persistence already
                    # succeeded and must not be reported as a failed ingest.
                    pass
