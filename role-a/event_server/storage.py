"""SQLite persistence for append-only Intent OS events."""

from __future__ import annotations

import json
import os
import sqlite3
import time
from datetime import date, datetime, time as datetime_time
from pathlib import Path
from typing import Any

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
        ingested_at = ingested_at or int(time.time())
        record = event.as_record()
        with self._connection() as connection:
            cursor = connection.execute(
                """
                INSERT INTO events(id, schema_version, ts, source, type, payload, ingested_at)
                VALUES (:id, :schema_version, :ts, :source, :type, :payload, :ingested_at)
                ON CONFLICT(id) DO NOTHING
                """,
                {**record, "payload": json.dumps(record["payload"], separators=(",", ":")), "ingested_at": ingested_at},
            )
            if cursor.rowcount:
                persisted = EventOut(**record, ingested_at=ingested_at)
                self._append_event_log(persisted)
                return True, persisted
            row = connection.execute("SELECT * FROM events WHERE id = ?", (record["id"],)).fetchone()
        return False, self._event_from_row(row)

    @staticmethod
    def _append_event_log(event: EventOut) -> None:
        """Best-effort JSONL for demo-day diagnosis; storage remains SQLite-first."""
        data_home = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local" / "share"))
        log_dir = data_home / "intent-os" / "logs"
        try:
            log_dir.mkdir(parents=True, exist_ok=True)
            with (log_dir / "events.jsonl").open("a", encoding="utf-8") as log:
                serializer = getattr(event, "model_dump_json", event.json)
                log.write(serializer() + "\n")
        except OSError:
            # A non-writable log directory must never stop local capture.
            pass

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


    def source_status(self) -> dict[str, dict[str, int]]:
        with self._connection() as connection:
            rows = connection.execute(
                "SELECT source, COUNT(*) AS event_count, MAX(ts) AS last_event_ts FROM events GROUP BY source ORDER BY source"
            ).fetchall()
        return {row["source"]: {"event_count": row["event_count"], "last_event_ts": row["last_event_ts"]} for row in rows}



    def detailed_event_counts(self) -> dict[str, int]:
        with self._connection() as connection:
            rows = connection.execute(
                """
                SELECT source || '/' || type AS kind, COUNT(*) AS event_count
                FROM events
                WHERE (source = 'vscode' AND type = 'document_change')
                   OR (source = 'firefox' AND type = 'user_action')
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
                """
            )
        return cursor.rowcount

def default_database_path() -> Path:
    configured = os.environ.get("INTENT_OS_DATABASE")
    if configured:
        return Path(configured).expanduser()
    data_home = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local" / "share"))
    return data_home / "intent-os" / "events.db"
