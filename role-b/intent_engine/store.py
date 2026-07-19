"""Async SQLite persistence, caching, and search for Role B intents."""

from __future__ import annotations

import json
import sqlite3
import time
from collections import OrderedDict, defaultdict
from contextlib import asynccontextmanager
from datetime import date as calendar_date
from pathlib import Path

import aiosqlite

from intent_engine.schemas import Intent, PipelineResult


class IntentStore:
    """Local Role B store; every intent node is persisted as an independent row."""

    def __init__(self, db_path: str = "intents.db") -> None:
        self.db_path = str(Path(db_path))
        self._fts_available = True
        self._search_cache: OrderedDict[tuple[str, int, str | None, str | None], tuple[dict, ...]] = OrderedDict()
        self.init_schema()

    def init_schema(self) -> None:
        """Create the database structure synchronously during construction."""

        path = Path(self.db_path)
        path.parent.mkdir(parents=True, exist_ok=True)
        connection = sqlite3.connect(self.db_path)
        try:
            connection.executescript(
                """
                CREATE TABLE IF NOT EXISTS pipeline_runs (
                    date TEXT,
                    source_hash TEXT,
                    status TEXT,
                    warnings_json TEXT,
                    started_at INTEGER,
                    completed_at INTEGER,
                    PRIMARY KEY (date, source_hash)
                );
                CREATE TABLE IF NOT EXISTS intents (
                    id TEXT PRIMARY KEY,
                    date TEXT,
                    parent_id TEXT NULL,
                    start_ts INTEGER,
                    end_ts INTEGER,
                    label TEXT,
                    summary TEXT,
                    depth INTEGER,
                    intent_json TEXT,
                    source_hash TEXT,
                    created_at INTEGER
                );
                CREATE INDEX IF NOT EXISTS idx_intents_date ON intents(date);
                CREATE INDEX IF NOT EXISTS idx_intents_parent_id ON intents(parent_id);
                """
            )
            try:
                connection.execute(
                    """
                    CREATE VIRTUAL TABLE IF NOT EXISTS intent_search USING fts5(
                        id UNINDEXED, label, summary, insights, tags
                    )
                    """
                )
            except sqlite3.OperationalError:
                self._fts_available = False
        finally:
            connection.close()

    async def cache_exists(self, date: str, source_hash: str) -> bool:
        async with self._connection() as connection:
            cursor = await connection.execute(
                "SELECT 1 FROM pipeline_runs WHERE date = ? AND source_hash = ?", (date, source_hash)
            )
            return await cursor.fetchone() is not None

    async def get_cached_intents(self, date: str, source_hash: str) -> list[Intent] | None:
        if not await self.cache_exists(date, source_hash):
            return None
        async with self._connection() as connection:
            rows = await self._fetch_rows(connection, date, source_hash)
        return self._rebuild_roots(rows)

    async def get_cached_warnings(self, date: str, source_hash: str) -> list[dict] | None:
        """Return persisted warning metadata for a cached pipeline run."""

        async with self._connection() as connection:
            cursor = await connection.execute(
                "SELECT warnings_json FROM pipeline_runs WHERE date = ? AND source_hash = ?", (date, source_hash)
            )
            row = await cursor.fetchone()
        return json.loads(row["warnings_json"]) if row is not None else None

    async def delete_date(self, date: str) -> dict:
        """Remove all cached pipeline data for one calendar date atomically."""

        async with self._connection() as connection:
            try:
                await connection.execute("BEGIN")
                cursor = await connection.execute("SELECT id FROM intents WHERE date = ? ORDER BY id", (date,))
                deleted_ids = [row["id"] for row in await cursor.fetchall()]
                if self._fts_available:
                    await connection.execute(
                        "DELETE FROM intent_search WHERE id IN (SELECT id FROM intents WHERE date = ?)", (date,)
                    )
                await connection.execute("DELETE FROM intents WHERE date = ?", (date,))
                await connection.execute("DELETE FROM pipeline_runs WHERE date = ?", (date,))
                await connection.commit()
                self._search_cache.clear()
                return {"deleted_intent_ids": deleted_ids, "deleted_count": len(deleted_ids)}
            except Exception:
                await connection.rollback()
                raise

    async def delete_project(self, project: str) -> dict:
        """Forget persisted intents whose tags identify the requested project."""

        project_name = project.strip()
        token = f"project:{project_name}"
        async with self._connection() as connection:
            try:
                await connection.execute("BEGIN")
                cursor = await connection.execute("SELECT id, date, intent_json FROM intents ORDER BY id")
                rows = await cursor.fetchall()
                matched = []
                affected_dates = set()
                for row in rows:
                    intent = Intent.model_validate_json(row["intent_json"])
                    if any(tag == project_name or token in tag for tag in intent.tags):
                        matched.append(row["id"])
                        affected_dates.add(row["date"])
                if matched:
                    placeholders = ",".join("?" for _ in matched)
                    if self._fts_available:
                        await connection.execute(f"DELETE FROM intent_search WHERE id IN ({placeholders})", matched)
                    await connection.execute(f"DELETE FROM intents WHERE id IN ({placeholders})", matched)
                for affected_date in affected_dates:
                    await connection.execute("DELETE FROM pipeline_runs WHERE date = ?", (affected_date,))
                await connection.commit()
                self._search_cache.clear()
                return {"deleted_intent_ids": matched, "deleted_count": len(matched)}
            except Exception:
                await connection.rollback()
                raise

    async def save_pipeline_run(self, date: str, result: PipelineResult) -> None:
        """Atomically replace a date's persisted intent tree and cache metadata."""

        now = int(time.time())
        warnings_json = json.dumps([warning.model_dump(mode="json") for warning in result.warnings], separators=(",", ":"))
        nodes = [node for root in result.intents for node in self._flatten(root)]

        async with self._connection() as connection:
            try:
                await connection.execute("BEGIN")
                if self._fts_available:
                    await connection.execute(
                        "DELETE FROM intent_search WHERE id IN (SELECT id FROM intents WHERE date = ?)", (date,)
                    )
                await connection.execute("DELETE FROM intents WHERE date = ?", (date,))
                await connection.execute("DELETE FROM pipeline_runs WHERE date = ?", (date,))
                await connection.execute(
                    """
                    INSERT INTO pipeline_runs(date, source_hash, status, warnings_json, started_at, completed_at)
                    VALUES (?, ?, 'complete', ?, ?, ?)
                    """,
                    (date, result.source_hash, warnings_json, now, now),
                )

                for node in nodes:
                    stored_node = node.model_copy(update={"children": []})
                    await connection.execute(
                        """
                        INSERT INTO intents(id, date, parent_id, start_ts, end_ts, label, summary, depth, intent_json, source_hash, created_at)
                        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                        """,
                        (
                            stored_node.id,
                            date,
                            stored_node.parent_id,
                            stored_node.start_ts,
                            stored_node.end_ts,
                            stored_node.label,
                            stored_node.summary,
                            stored_node.depth,
                            stored_node.model_dump_json(),
                            result.source_hash,
                            now,
                        ),
                    )
                    if self._fts_available:
                        await connection.execute(
                            "INSERT INTO intent_search(id, label, summary, insights, tags) VALUES (?, ?, ?, ?, ?)",
                            (
                                stored_node.id,
                                stored_node.label,
                                stored_node.summary,
                                self._searchable_evidence(stored_node),
                                json.dumps(stored_node.tags, separators=(",", ":")),
                            ),
                        )
                await connection.commit()
                self._search_cache.clear()
            except Exception:
                await connection.rollback()
                raise

    async def get_intents_by_date(self, date: str) -> list[Intent]:
        async with self._connection() as connection:
            rows = await self._fetch_rows(connection, date)
        return self._rebuild_roots(rows)

    async def get_intent_stats(
        self,
        date_from: str,
        date_to: str,
        project: str | None = None,
    ) -> dict:
        """Aggregate persisted root-intent metadata for an inclusive date range."""

        date_from = self._validate_search_date(date_from, "date_from")
        date_to = self._validate_search_date(date_to, "date_to")
        if date_from is None or date_to is None or date_from > date_to:
            raise ValueError("date_from must not be later than date_to")

        async with self._connection() as connection:
            cursor = await connection.execute(
                "SELECT intent_json FROM intents WHERE date >= ? AND date <= ? AND depth = 0 ORDER BY date, id",
                (date_from, date_to),
            )
            roots = [Intent.model_validate_json(row["intent_json"]) for row in await cursor.fetchall()]

        project_name = project.strip() if project is not None else None
        if project_name:
            token = f"project:{project_name}"
            roots = [
                root for root in roots
                if any(tag == project_name or token in tag for tag in root.tags)
            ]

        by_date: dict[str, dict[str, int]] = {}
        label_counts: dict[str, int] = {}
        project_counts: dict[str, int] = {}
        total_duration = 0
        event_count = 0
        for root in roots:
            bucket = by_date.setdefault(root.date, {"intent_count": 0, "duration_seconds": 0})
            bucket["intent_count"] += 1
            bucket["duration_seconds"] += root.stats.duration_seconds
            total_duration += root.stats.duration_seconds
            event_count += root.stats.event_count
            label_counts[root.label] = label_counts.get(root.label, 0) + 1
            for tag in root.tags:
                project_counts[tag] = project_counts.get(tag, 0) + 1

        return {
            "date_from": date_from,
            "date_to": date_to,
            "project": project_name or None,
            "intent_count": len(roots),
            "total_duration_seconds": total_duration,
            "event_count": event_count,
            "by_date": [
                {"date": day, **by_date[day]} for day in sorted(by_date)
            ],
            "top_labels": [
                {"label": label, "count": count}
                for label, count in sorted(label_counts.items(), key=lambda item: (-item[1], item[0]))
            ],
            "projects": [
                {"tag": tag, "count": count}
                for tag, count in sorted(project_counts.items(), key=lambda item: (-item[1], item[0]))
            ],
        }

    async def get_intent_by_id(self, intent_id: str) -> Intent | None:
        async with self._connection() as connection:
            cursor = await connection.execute(
                "SELECT date, source_hash, parent_id, intent_json FROM intents WHERE id = ?", (intent_id,)
            )
            row = await cursor.fetchone()
            if row is None:
                return None
            if row["parent_id"] is not None:
                return Intent.model_validate_json(row["intent_json"])
            rows = await self._fetch_rows(connection, row["date"], row["source_hash"])
        return next((intent for intent in self._rebuild_roots(rows) if intent.id == intent_id), None)

    async def search_intents(
        self,
        query: str,
        limit: int = 10,
        *,
        date_from: str | None = None,
        date_to: str | None = None,
    ) -> list[dict]:
        normalized_query = query.strip()
        safe_limit = max(1, min(limit, 100))
        date_from = self._validate_search_date(date_from, "date_from")
        date_to = self._validate_search_date(date_to, "date_to")
        if date_from and date_to and date_from > date_to:
            raise ValueError("date_from must not be later than date_to")
        cache_key = (normalized_query.casefold(), safe_limit, date_from, date_to)
        cached = self._search_cache.get(cache_key)
        if cached is not None:
            self._search_cache.move_to_end(cache_key)
            return [dict(record) for record in cached]
        if not normalized_query:
            return []

        async with self._connection() as connection:
            if self._fts_available:
                try:
                    conditions = [
                        "(intent_search.label MATCH ? OR intent_search.summary MATCH ? OR "
                        "intent_search.insights MATCH ? OR intent_search.tags MATCH ?)",
                        "intents.depth = 0",
                    ]
                    params: list[object] = [normalized_query] * 4
                    if date_from:
                        conditions.append("intents.date >= ?")
                        params.append(date_from)
                    if date_to:
                        conditions.append("intents.date <= ?")
                        params.append(date_to)
                    params.append(safe_limit)
                    cursor = await connection.execute(
                        f"""
                        SELECT intents.id, intents.label, intents.summary, intents.date
                        FROM intent_search
                        JOIN intents ON intents.id = intent_search.id
                        WHERE {' AND '.join(conditions)}
                        LIMIT ?
                        """,
                        params,
                    )
                    rows = await cursor.fetchall()
                    return self._cache_search_results(
                        cache_key, [self._search_record(row, normalized_query) for row in rows]
                    )
                except sqlite3.OperationalError:
                    pass

            pattern = f"%{normalized_query}%"
            conditions = [
                "(label LIKE ? OR summary LIKE ? OR intent_json LIKE ? OR label LIKE ?)",
                "depth = 0",
            ]
            params = [pattern, pattern, pattern, pattern]
            if date_from:
                conditions.append("date >= ?")
                params.append(date_from)
            if date_to:
                conditions.append("date <= ?")
                params.append(date_to)
            params.append(safe_limit)
            cursor = await connection.execute(
                f"""
                SELECT id, label, summary, date FROM intents
                WHERE {' AND '.join(conditions)}
                LIMIT ?
                """,
                params,
            )
            return self._cache_search_results(
                cache_key, [self._search_record(row, normalized_query) for row in await cursor.fetchall()]
            )

    @asynccontextmanager
    async def _connection(self):
        connection = await aiosqlite.connect(self.db_path)
        connection.row_factory = aiosqlite.Row
        try:
            yield connection
        finally:
            await connection.close()

    @staticmethod
    async def _fetch_rows(connection: aiosqlite.Connection, date: str, source_hash: str | None = None):
        if source_hash is None:
            cursor = await connection.execute(
                "SELECT intent_json FROM intents WHERE date = ? ORDER BY start_ts, id", (date,)
            )
        else:
            cursor = await connection.execute(
                "SELECT intent_json FROM intents WHERE date = ? AND source_hash = ? ORDER BY start_ts, id",
                (date, source_hash),
            )
        return await cursor.fetchall()

    @staticmethod
    def _flatten(intent: Intent) -> list[Intent]:
        return [intent, *(node for child in intent.children for node in IntentStore._flatten(child))]

    @staticmethod
    def _rebuild_roots(rows) -> list[Intent]:
        nodes = [Intent.model_validate_json(row["intent_json"]) for row in rows]
        by_id = {node.id: node.model_copy(update={"children": []}) for node in nodes}
        children: dict[str, list[Intent]] = defaultdict(list)
        roots: list[Intent] = []
        for node in by_id.values():
            if node.parent_id and node.parent_id in by_id:
                children[node.parent_id].append(node)
            elif node.parent_id is None:
                roots.append(node)

        def attach(node: Intent) -> Intent:
            ordered_children = sorted(children.get(node.id, []), key=lambda child: (child.start_ts, child.id))
            return node.model_copy(update={"children": [attach(child) for child in ordered_children]})

        return [attach(root) for root in sorted(roots, key=lambda root: (root.start_ts, root.id))]

    def _cache_search_results(self, cache_key: tuple[str, int, str | None, str | None], records: list[dict]) -> list[dict]:
        self._search_cache[cache_key] = tuple(dict(record) for record in records)
        self._search_cache.move_to_end(cache_key)
        while len(self._search_cache) > 128:
            self._search_cache.popitem(last=False)
        return [dict(record) for record in self._search_cache[cache_key]]

    @staticmethod
    def _validate_search_date(value: str | None, field: str) -> str | None:
        if value is None:
            return None
        try:
            parsed = calendar_date.fromisoformat(value)
        except (TypeError, ValueError) as exc:
            raise ValueError(f"{field} must be a real YYYY-MM-DD calendar date") from exc
        if parsed.isoformat() != value:
            raise ValueError(f"{field} must be a real YYYY-MM-DD calendar date")
        return value

    @staticmethod
    def _search_record(row, query: str) -> dict:
        label = row["label"]
        summary = row["summary"]
        return {
            "id": row["id"],
            "label": label,
            "summary": summary,
            "date": row["date"],
            "highlight_snippet": IntentStore._highlight_snippet(label, summary, query),
        }

    @staticmethod
    def _highlight_snippet(label: str, summary: str, query: str) -> str:
        for text in (summary, label):
            index = text.lower().find(query.lower())
            if index >= 0:
                width = max(80, len(query))
                start = max(0, index - (width - len(query)) // 2)
                end = min(len(text), start + width)
                start = max(0, end - width)
                excerpt = text[start:end]
                match_start = index - start
                highlighted = (
                    f"{excerpt[:match_start]}**{excerpt[match_start:match_start + len(query)]}**"
                    f"{excerpt[match_start + len(query):]}"
                )
                return f"{'...' if start else ''}{highlighted}{'...' if end < len(text) else ''}"
        return f"{summary[:80]}{'...' if len(summary) > 80 else ''}"

    @staticmethod
    def _searchable_evidence(intent: Intent) -> str:
        """Index Role-A-approved evidence alongside derived insights.

        The full structured evidence remains in ``intent_json``; this compact
        text makes it discoverable by the FTS path used by Copilot search.
        """

        insights = json.dumps(intent.insights.model_dump(mode="json"), separators=(",", ":"))
        evidence = " ".join(f"{item.field} {item.value}" for item in intent.evidence)
        return f"{insights} {evidence}".strip()
