"""SQLite persistence for append-only Intent OS events."""

from __future__ import annotations

import json
import os
import sqlite3
import time
from datetime import date, datetime, time as datetime_time
from pathlib import Path
from typing import Any, Iterable

from .models import DayExport, EventIn, EventOut


class EventStore:
    def __init__(self, database_path: Path) -> None:
        self.database_path = database_path
        self.database_path.parent.mkdir(parents=True, exist_ok=True)
        self._migrate()

    def _connection(self) -> sqlite3.Connection:
        connection = sqlite3.connect(self.database_path, timeout=5)
        connection.row_factory = sqlite3.Row
        connection.execute("PRAGMA busy_timeout = 5000")
        return connection

    def _migrate(self) -> None:
        with self._connection() as connection:
            connection.executescript(
                """
                PRAGMA journal_mode = WAL;
                CREATE TABLE IF NOT EXISTS schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS events (
                    id TEXT PRIMARY KEY,
                    schema_version INTEGER NOT NULL DEFAULT 1,
                    ts INTEGER NOT NULL,
                    source TEXT NOT NULL,
                    type TEXT NOT NULL,
                    payload TEXT NOT NULL,
                    ingested_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_events_ts ON events(ts, ingested_at);
                CREATE INDEX IF NOT EXISTS idx_events_source_type_ts ON events(source, type, ts);
                """
            )
            columns = {row[1] for row in connection.execute("PRAGMA table_info(events)")}
            if "schema_version" not in columns:
                connection.execute("ALTER TABLE events ADD COLUMN schema_version INTEGER NOT NULL DEFAULT 1")
            connection.execute(
                "INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES (?, ?)",
                (1, int(time.time())),
            )

    def insert(self, event: EventIn, ingested_at: int | None = None) -> tuple[bool, EventOut]:
        """Persist one event through the same atomic path used by the writer."""

        return self.insert_many([(event, ingested_at or int(time.time()))])[0]

    def insert_many(self, records: Iterable[tuple[EventIn, int]]) -> list[tuple[bool, EventOut]]:
        """Append a batch atomically while preserving per-event idempotency.

        Role A uses UUIDs as collector retry keys.  A duplicate in the same
        batch or a later batch returns the original stored row, exactly like
        ``insert`` did before batching was introduced.
        """

        batch = list(records)
        if not batch:
            return []

        connection = self._connection()
        try:
            connection.execute("BEGIN IMMEDIATE")
            results: list[tuple[bool, EventOut]] = []
            for event, ingested_at in batch:
                record = event.as_record()
                cursor = connection.execute(
                    """
                    INSERT INTO events(id, schema_version, ts, source, type, payload, ingested_at)
                    VALUES (:id, :schema_version, :ts, :source, :type, :payload, :ingested_at)
                    ON CONFLICT(id) DO NOTHING
                    """,
                    {
                        **record,
                        "payload": json.dumps(record["payload"], separators=(",", ":")),
                        "ingested_at": ingested_at,
                    },
                )
                if cursor.rowcount:
                    results.append((True, EventOut(**record, ingested_at=ingested_at)))
                    continue
                row = connection.execute("SELECT * FROM events WHERE id = ?", (record["id"],)).fetchone()
                if row is None:
                    raise RuntimeError("duplicate event could not be loaded")
                results.append((False, self._event_from_row(row)))
            connection.commit()
            return results
        except Exception:
            connection.rollback()
            raise
        finally:
            connection.close()

    @staticmethod
    def _event_from_row(row: sqlite3.Row) -> EventOut:
        return EventOut(
            id=row["id"],
            schema_version=row["schema_version"],
            ts=row["ts"],
            source=row["source"],
            type=row["type"],
            payload=json.loads(row["payload"]),
            ingested_at=row["ingested_at"],
        )

    def list_events(self, *, date_value: str | None = None, since: int | None = None) -> list[EventOut]:
        if date_value and since is not None:
            raise ValueError("date and since cannot be used together")

        query = "SELECT * FROM events"
        params: tuple[Any, ...] = ()
        if date_value:
            start, end = self._date_bounds(date_value)
            query += " WHERE ts >= ? AND ts < ?"
            params = (start, end)
        elif since is not None:
            query += " WHERE ts >= ?"
            params = (since,)
        query += " ORDER BY ts ASC, ingested_at ASC"

        with self._connection() as connection:
            rows = connection.execute(query, params).fetchall()
        return [self._event_from_row(row) for row in rows]

    @staticmethod
    def _date_bounds(date_value: str) -> tuple[int, int]:
        try:
            target = date.fromisoformat(date_value)
        except ValueError as exc:
            raise ValueError("date must be YYYY-MM-DD") from exc
        local_tz = datetime.now().astimezone().tzinfo
        start = datetime.combine(target, datetime_time.min, tzinfo=local_tz)
        end = datetime.combine(target.fromordinal(target.toordinal() + 1), datetime_time.min, tzinfo=local_tz)
        return int(start.timestamp()), int(end.timestamp())

    def export_day(self, date_value: str, exported_at: int | None = None) -> DayExport:
        return DayExport(
            date=date_value,
            exported_at=exported_at or int(time.time()),
            events=self.list_events(date_value=date_value),
        )


    def source_status(
        self, *, now: int | None = None, stale_after_seconds: int = 30 * 60
    ) -> dict[str, dict[str, int | bool | None]]:
        if stale_after_seconds <= 0:
            raise ValueError("stale_after_seconds must be positive")
        now = int(time.time()) if now is None else now
        with self._connection() as connection:
            rows = connection.execute(
                """
                SELECT source, COUNT(*) AS event_count, MAX(ts) AS last_event_ts,
                       MAX(ingested_at) AS last_ingested_at
                FROM events
                GROUP BY source
                ORDER BY source
                """
            ).fetchall()
        result: dict[str, dict[str, int | bool | None]] = {
            source: {
                "event_count": 0,
                "last_event_ts": None,
                "last_ingested_at": None,
                "last_ingest_lag_seconds": None,
                "healthy": False,
            }
            for source in ("vscode", "firefox", "shell", "linux", "filesystem")
        }
        for row in rows:
            last_event_ts = row["last_event_ts"]
            last_ingested_at = row["last_ingested_at"]
            result[row["source"]] = {
                "event_count": row["event_count"],
                "last_event_ts": last_event_ts,
                "last_ingested_at": last_ingested_at,
                "last_ingest_lag_seconds": max(0, last_ingested_at - last_event_ts)
                if last_event_ts is not None and last_ingested_at is not None
                else None,
                "healthy": last_event_ts is not None and now - last_event_ts <= stale_after_seconds,
            }
        return result



    def detailed_event_counts(self) -> dict[str, int]:
        with self._connection() as connection:
            rows = connection.execute(
                """
                SELECT source || '/' || type AS kind, COUNT(*) AS event_count
                FROM events
                WHERE (source = 'vscode' AND type = 'document_change')
                   OR (source = 'firefox' AND type = 'user_action')
                   OR (source = 'filesystem' AND type = 'file_content')
                GROUP BY source, type
                ORDER BY source, type
                """
            ).fetchall()
        return {row["kind"]: row["event_count"] for row in rows}

    def purge_detailed_events(self) -> int:
        with self._connection() as connection:
            cursor = connection.execute(
                """
                DELETE FROM events
                WHERE (source = 'vscode' AND type = 'document_change')
                   OR (source = 'firefox' AND type = 'user_action')
                   OR (source = 'filesystem' AND type = 'file_content')
                """
            )
        return cursor.rowcount

    def retention_preview(
        self,
        *,
        detailed_days: int | None = None,
        metadata_days: int | None = None,
        now: int | None = None,
    ) -> dict[str, object]:
        """Count expired raw records without deleting them.

        Detailed records are the opt-in editor/browser/filesystem payloads.
        Metadata means every other Role A event.  Detailed records honour the
        shortest configured window so a broad metadata cutoff cannot retain a
        sensitive record longer than requested.
        """

        now = int(time.time()) if now is None else now
        detailed_cutoff, metadata_cutoff = self._retention_cutoffs(
            detailed_days=detailed_days,
            metadata_days=metadata_days,
            now=now,
        )
        detailed_where, detailed_params = self._detailed_retention_where(detailed_cutoff, metadata_cutoff)
        metadata_where, metadata_params = self._metadata_retention_where(metadata_cutoff)
        with self._connection() as connection:
            detailed_count = connection.execute(
                f"SELECT COUNT(*) FROM events WHERE {detailed_where}", detailed_params
            ).fetchone()[0]
            metadata_count = connection.execute(
                f"SELECT COUNT(*) FROM events WHERE {metadata_where}", metadata_params
            ).fetchone()[0]
        return {
            "now": now,
            "detailed_days": detailed_days,
            "metadata_days": metadata_days,
            "cutoffs": {"detailed_before": detailed_cutoff, "metadata_before": metadata_cutoff},
            "eligible": {
                "detailed": detailed_count,
                "metadata": metadata_count,
                "total": detailed_count + metadata_count,
            },
        }

    def purge_retention(
        self,
        *,
        detailed_days: int | None = None,
        metadata_days: int | None = None,
        now: int | None = None,
    ) -> dict[str, object]:
        """Delete only records selected by an explicit tiered retention policy."""

        preview = self.retention_preview(
            detailed_days=detailed_days,
            metadata_days=metadata_days,
            now=now,
        )
        detailed_cutoff = preview["cutoffs"]["detailed_before"]
        metadata_cutoff = preview["cutoffs"]["metadata_before"]
        detailed_where, detailed_params = self._detailed_retention_where(detailed_cutoff, metadata_cutoff)
        metadata_where, metadata_params = self._metadata_retention_where(metadata_cutoff)
        with self._connection() as connection:
            detailed_deleted = connection.execute(
                f"DELETE FROM events WHERE {detailed_where}", detailed_params
            ).rowcount
            metadata_deleted = connection.execute(
                f"DELETE FROM events WHERE {metadata_where}", metadata_params
            ).rowcount
        return {
            **preview,
            "deleted": {
                "detailed": detailed_deleted,
                "metadata": metadata_deleted,
                "total": detailed_deleted + metadata_deleted,
            },
        }

    @staticmethod
    def _retention_cutoffs(
        *, detailed_days: int | None, metadata_days: int | None, now: int
    ) -> tuple[int | None, int | None]:
        for name, value in (("detailed_days", detailed_days), ("metadata_days", metadata_days)):
            if value is not None and (isinstance(value, bool) or not isinstance(value, int) or value < 1):
                raise ValueError(f"{name} must be a positive integer or null")
        return (
            now - detailed_days * 86_400 if detailed_days is not None else None,
            now - metadata_days * 86_400 if metadata_days is not None else None,
        )

    @staticmethod
    def _detailed_event_clause() -> str:
        return """(
            (source = 'vscode' AND type = 'document_change')
            OR (source = 'firefox' AND type = 'user_action')
            OR (source = 'filesystem' AND type = 'file_content')
        )"""

    @classmethod
    def _detailed_retention_where(
        cls, detailed_cutoff: object, metadata_cutoff: object
    ) -> tuple[str, tuple[object, ...]]:
        cutoffs = [cutoff for cutoff in (detailed_cutoff, metadata_cutoff) if isinstance(cutoff, int)]
        if not cutoffs:
            return "0", ()
        # A shorter retention period has a later cutoff timestamp.  Detailed
        # events must expire when either applicable policy says so, therefore
        # use the later cutoff rather than retaining them until both expire.
        return f"{cls._detailed_event_clause()} AND ts < ?", (max(cutoffs),)

    @classmethod
    def _metadata_retention_where(cls, metadata_cutoff: object) -> tuple[str, tuple[object, ...]]:
        if not isinstance(metadata_cutoff, int):
            return "0", ()
        return f"NOT {cls._detailed_event_clause()} AND ts < ?", (metadata_cutoff,)

def default_database_path() -> Path:
    configured = os.environ.get("INTENT_OS_DATABASE")
    if configured:
        return Path(configured).expanduser()
    data_home = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local" / "share"))
    return data_home / "intent-os" / "events.db"
