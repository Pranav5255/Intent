"""SQLite persistence for append-only Intent events."""

from __future__ import annotations

import json
import os
import sqlite3
import time
from datetime import date, datetime, time as datetime_time
from pathlib import Path
from typing import Any, Iterable

from .models import DayExport, DayExportMeta, EventCursor, EventIn, EventOut, IncrementalEventsPage, IncrementalPageMeta


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
                CREATE TABLE IF NOT EXISTS day_export_stats (
                    date TEXT PRIMARY KEY,
                    event_count INTEGER NOT NULL,
                    first_ts INTEGER,
                    last_ts INTEGER,
                    last_event_id TEXT,
                    revision INTEGER NOT NULL DEFAULT 0,
                    updated_at INTEGER NOT NULL
                );
                """
            )
            columns = {row[1] for row in connection.execute("PRAGMA table_info(events)")}
            if "schema_version" not in columns:
                connection.execute("ALTER TABLE events ADD COLUMN schema_version INTEGER NOT NULL DEFAULT 1")
            connection.execute(
                "INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES (?, ?)",
                (1, int(time.time())),
            )
            migrated = connection.execute(
                "SELECT 1 FROM schema_migrations WHERE version = ?", (2,)
            ).fetchone()
            if migrated is None:
                self._backfill_day_export_stats(connection)
                connection.execute(
                    "INSERT INTO schema_migrations(version, applied_at) VALUES (?, ?)",
                    (2, int(time.time())),
                )

    @staticmethod
    def _local_tz():
        return datetime.now().astimezone().tzinfo

    @classmethod
    def _local_date_for_ts(cls, ts: int) -> str:
        return datetime.fromtimestamp(ts, tz=cls._local_tz()).date().isoformat()

    @staticmethod
    def _content_hash(revision: int, event_count: int, last_event_id: str | None) -> str:
        last = last_event_id or ""
        return f"rev:{revision}|count:{event_count}|last:{last}"

    @staticmethod
    def _is_after_ts_id(ts: int, event_id: str, after_ts: int, after_id: str) -> bool:
        return ts > after_ts or (ts == after_ts and event_id > after_id)

    @classmethod
    def _is_newer_event(
        cls,
        ts: int,
        event_id: str,
        last_ts: int | None,
        last_event_id: str | None,
    ) -> bool:
        if last_ts is None or last_event_id is None:
            return True
        return cls._is_after_ts_id(ts, event_id, last_ts, last_event_id)

    @classmethod
    def _apply_day_insert(
        cls,
        connection: sqlite3.Connection,
        *,
        date_value: str,
        event_id: str,
        ts: int,
        ingested_at: int,
        now: int,
    ) -> None:
        row = connection.execute(
            "SELECT event_count, first_ts, last_ts, last_event_id, revision FROM day_export_stats WHERE date = ?",
            (date_value,),
        ).fetchone()
        if row is None:
            connection.execute(
                """
                INSERT INTO day_export_stats(
                    date, event_count, first_ts, last_ts, last_event_id, revision, updated_at
                ) VALUES (?, 1, ?, ?, ?, 1, ?)
                """,
                (date_value, ts, ts, event_id, now),
            )
            return

        first_ts = row["first_ts"]
        if first_ts is None or ts < first_ts:
            first_ts = ts
        last_ts = row["last_ts"]
        last_event_id = row["last_event_id"]
        if cls._is_newer_event(ts, event_id, last_ts, last_event_id):
            last_ts = ts
            last_event_id = event_id
        connection.execute(
            """
            UPDATE day_export_stats
            SET event_count = event_count + 1,
                first_ts = ?,
                last_ts = ?,
                last_event_id = ?,
                revision = revision + 1,
                updated_at = ?
            WHERE date = ?
            """,
            (first_ts, last_ts, last_event_id, now, date_value),
        )

    @classmethod
    def _recompute_day_stats(cls, connection: sqlite3.Connection, date_value: str) -> None:
        start, end = cls._date_bounds(date_value)
        now = int(time.time())
        row = connection.execute(
            """
            SELECT COUNT(*) AS event_count, MIN(ts) AS first_ts
            FROM events
            WHERE ts >= ? AND ts < ?
            """,
            (start, end),
        ).fetchone()
        event_count = int(row["event_count"])
        if event_count == 0:
            connection.execute("DELETE FROM day_export_stats WHERE date = ?", (date_value,))
            return

        last_row = connection.execute(
            """
            SELECT id, ts FROM events
            WHERE ts >= ? AND ts < ?
            ORDER BY ts DESC, ingested_at DESC, id DESC
            LIMIT 1
            """,
            (start, end),
        ).fetchone()
        existing = connection.execute(
            "SELECT revision FROM day_export_stats WHERE date = ?", (date_value,)
        ).fetchone()
        revision = int(existing["revision"]) + 1 if existing is not None else 1
        connection.execute(
            """
            INSERT INTO day_export_stats(
                date, event_count, first_ts, last_ts, last_event_id, revision, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(date) DO UPDATE SET
                event_count = excluded.event_count,
                first_ts = excluded.first_ts,
                last_ts = excluded.last_ts,
                last_event_id = excluded.last_event_id,
                revision = excluded.revision,
                updated_at = excluded.updated_at
            """,
            (
                date_value,
                event_count,
                row["first_ts"],
                last_row["ts"],
                last_row["id"],
                revision,
                now,
            ),
        )

    @classmethod
    def _backfill_day_export_stats(cls, connection: sqlite3.Connection) -> None:
        rows = connection.execute("SELECT id, ts, ingested_at FROM events ORDER BY ts ASC, ingested_at ASC, id ASC").fetchall()
        if not rows:
            return
        now = int(time.time())
        by_date: dict[str, list[sqlite3.Row]] = {}
        for row in rows:
            date_value = cls._local_date_for_ts(int(row["ts"]))
            by_date.setdefault(date_value, []).append(row)
        for date_value, day_rows in by_date.items():
            first_ts = int(day_rows[0]["ts"])
            last = day_rows[-1]
            connection.execute(
                """
                INSERT INTO day_export_stats(
                    date, event_count, first_ts, last_ts, last_event_id, revision, updated_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?)
                """,
                (date_value, len(day_rows), first_ts, int(last["ts"]), str(last["id"]), 1, now),
            )

    @classmethod
    def _affected_dates_for_ts_range(cls, connection: sqlite3.Connection, min_ts: int | None, max_ts: int | None) -> set[str]:
        if min_ts is None or max_ts is None:
            return set()
        rows = connection.execute(
            "SELECT DISTINCT ts FROM events WHERE ts >= ? AND ts <= ?",
            (min_ts, max_ts),
        ).fetchall()
        return {cls._local_date_for_ts(int(row["ts"])) for row in rows}

    def insert(self, event: EventIn, ingested_at: int | None = None) -> tuple[bool, EventOut]:
        """Persist one event through the same atomic path used by the writer."""

        return self.insert_many([(event, ingested_at or int(time.time()))])[0]

    def insert_many(self, records: Iterable[tuple[EventIn, int]]) -> list[tuple[bool, EventOut]]:
        """Append a batch atomically while preserving per-event idempotency."""

        batch = list(records)
        if not batch:
            return []

        connection = self._connection()
        try:
            connection.execute("BEGIN IMMEDIATE")
            results: list[tuple[bool, EventOut]] = []
            now = int(time.time())
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
                    date_value = self._local_date_for_ts(record["ts"])
                    self._apply_day_insert(
                        connection,
                        date_value=date_value,
                        event_id=record["id"],
                        ts=record["ts"],
                        ingested_at=ingested_at,
                        now=now,
                    )
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

    _EVENT_ORDER = " ORDER BY ts ASC, ingested_at ASC, id ASC"

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
        query += self._EVENT_ORDER

        with self._connection() as connection:
            rows = connection.execute(query, params).fetchall()
        return [self._event_from_row(row) for row in rows]

    def list_events_incremental(
        self,
        date_value: str,
        *,
        after_ts: int | None = None,
        after_id: str | None = None,
        limit: int = 500,
    ) -> IncrementalEventsPage:
        if limit <= 0 or limit > 2000:
            raise ValueError("limit must be between 1 and 2000")
        if (after_ts is None) != (after_id is None):
            raise ValueError("after_ts and after_id must be provided together")

        start, end = self._date_bounds(date_value)
        meta = self.export_day_meta(date_value)
        query = "SELECT * FROM events WHERE ts >= ? AND ts < ?"
        params: list[Any] = [start, end]
        if after_ts is not None and after_id is not None:
            query += " AND (ts > ? OR (ts = ? AND id > ?))"
            params.extend([after_ts, after_ts, after_id])
        query += self._EVENT_ORDER + " LIMIT ?"
        params.append(limit + 1)

        with self._connection() as connection:
            rows = connection.execute(query, params).fetchall()

        has_more = len(rows) > limit
        page_rows = rows[:limit]
        events = [self._event_from_row(row) for row in page_rows]
        next_cursor = None
        if has_more and page_rows:
            last = page_rows[-1]
            next_cursor = EventCursor(after_ts=int(last["ts"]), after_id=str(last["id"]))

        return IncrementalEventsPage(
            date=date_value,
            events=events,
            has_more=has_more,
            next_cursor=next_cursor,
            page_meta=IncrementalPageMeta(revision=meta.revision, content_hash=meta.content_hash),
        )

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

    def export_day_meta(self, date_value: str, checked_at: int | None = None) -> DayExportMeta:
        self._date_bounds(date_value)
        with self._connection() as connection:
            row = connection.execute(
                """
                SELECT event_count, first_ts, last_ts, last_event_id, revision
                FROM day_export_stats
                WHERE date = ?
                """,
                (date_value,),
            ).fetchone()

        if row is None:
            return DayExportMeta(
                date=date_value,
                event_count=0,
                revision=0,
                content_hash=self._content_hash(0, 0, None),
                checked_at=checked_at or int(time.time()),
            )

        revision = int(row["revision"])
        event_count = int(row["event_count"])
        last_event_id = row["last_event_id"]
        return DayExportMeta(
            date=date_value,
            event_count=event_count,
            first_ts=row["first_ts"],
            last_ts=row["last_ts"],
            last_event_id=last_event_id,
            revision=revision,
            content_hash=self._content_hash(revision, event_count, last_event_id),
            checked_at=checked_at or int(time.time()),
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
        connection = self._connection()
        try:
            connection.execute("BEGIN IMMEDIATE")
            bounds = connection.execute(
                f"""
                SELECT MIN(ts) AS min_ts, MAX(ts) AS max_ts
                FROM events
                WHERE {self._detailed_event_clause()}
                """
            ).fetchone()
            affected = self._affected_dates_for_ts_range(
                connection,
                bounds["min_ts"],
                bounds["max_ts"],
            )
            cursor = connection.execute(
                f"""
                DELETE FROM events
                WHERE {self._detailed_event_clause()}
                """
            )
            deleted = cursor.rowcount
            for date_value in affected:
                self._recompute_day_stats(connection, date_value)
            connection.commit()
            return deleted
        except Exception:
            connection.rollback()
            raise
        finally:
            connection.close()

    def retention_preview(
        self,
        *,
        detailed_days: int | None = None,
        metadata_days: int | None = None,
        now: int | None = None,
    ) -> dict[str, object]:
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
        preview = self.retention_preview(
            detailed_days=detailed_days,
            metadata_days=metadata_days,
            now=now,
        )
        detailed_cutoff = preview["cutoffs"]["detailed_before"]
        metadata_cutoff = preview["cutoffs"]["metadata_before"]
        detailed_where, detailed_params = self._detailed_retention_where(detailed_cutoff, metadata_cutoff)
        metadata_where, metadata_params = self._metadata_retention_where(metadata_cutoff)
        connection = self._connection()
        try:
            connection.execute("BEGIN IMMEDIATE")
            affected: set[str] = set()
            for where, params in ((detailed_where, detailed_params), (metadata_where, metadata_params)):
                if where == "0":
                    continue
                bounds = connection.execute(
                    f"SELECT MIN(ts) AS min_ts, MAX(ts) AS max_ts FROM events WHERE {where}",
                    params,
                ).fetchone()
                affected.update(self._affected_dates_for_ts_range(connection, bounds["min_ts"], bounds["max_ts"]))
            detailed_deleted = connection.execute(
                f"DELETE FROM events WHERE {detailed_where}", detailed_params
            ).rowcount
            metadata_deleted = connection.execute(
                f"DELETE FROM events WHERE {metadata_where}", metadata_params
            ).rowcount
            for date_value in affected:
                self._recompute_day_stats(connection, date_value)
            connection.commit()
        except Exception:
            connection.rollback()
            raise
        finally:
            connection.close()
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
        return f"{cls._detailed_event_clause()} AND ts < ?", (max(cutoffs),)

    @classmethod
    def _metadata_retention_where(cls, metadata_cutoff: object) -> tuple[str, tuple[object, ...]]:
        if not isinstance(metadata_cutoff, int):
            return "0", ()
        return f"NOT {cls._detailed_event_clause()} AND ts < ?", (metadata_cutoff,)


def default_database_path() -> Path:
    configured = os.environ.get("INTENT_DATABASE")
    if configured:
        return Path(configured).expanduser()
    data_home = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local" / "share"))
    return data_home / "intent" / "events.db"
