"""Per-day ingest fingerprints and incremental export orchestration."""

from __future__ import annotations

import json
import os
import time
from dataclasses import dataclass
from datetime import date as calendar_date

from intent_engine.schemas import DayExport, DayExportMeta, EventCursor, RawEvent
from intent_engine.source import RoleAClient
from intent_engine.store import IntentStore


INGEST_DAY_KEY_PREFIX = "ingest_day:"
INGEST_OPEN_DATE_KEY = "ingest_open_date"


@dataclass(frozen=True)
class DayIngestState:
    content_hash: str
    revision: int
    event_count: int
    last_event_id: str | None
    processed_at: int
    source_hash: str | None = None
    cursor_ts: int | None = None
    cursor_id: str | None = None

    def to_json(self) -> str:
        return json.dumps({
            "content_hash": self.content_hash,
            "revision": self.revision,
            "event_count": self.event_count,
            "last_event_id": self.last_event_id,
            "processed_at": self.processed_at,
            "source_hash": self.source_hash,
            "cursor_ts": self.cursor_ts,
            "cursor_id": self.cursor_id,
        }, separators=(",", ":"))

    @classmethod
    def from_json(cls, raw: str) -> DayIngestState:
        data = json.loads(raw)
        return cls(
            content_hash=str(data["content_hash"]),
            revision=int(data["revision"]),
            event_count=int(data["event_count"]),
            last_event_id=data.get("last_event_id"),
            processed_at=int(data["processed_at"]),
            source_hash=data.get("source_hash"),
            cursor_ts=data.get("cursor_ts"),
            cursor_id=data.get("cursor_id"),
        )


def ingest_day_key(date: str) -> str:
    return f"{INGEST_DAY_KEY_PREFIX}{date}"


async def load_day_state(store: IntentStore, date: str) -> DayIngestState | None:
    raw = await store.get_metadata(ingest_day_key(date))
    if raw is None:
        return None
    try:
        return DayIngestState.from_json(raw)
    except (TypeError, ValueError, json.JSONDecodeError, KeyError):
        return None


async def save_day_state(store: IntentStore, date: str, state: DayIngestState) -> None:
    await store.set_metadata(ingest_day_key(date), state.to_json())


def should_skip(meta: DayExportMeta, state: DayIngestState | None) -> bool:
    if state is None:
        return False
    return state.content_hash == meta.content_hash


async def mark_skipped(store: IntentStore, date: str, meta: DayExportMeta, state: DayIngestState | None) -> None:
    processed_at = int(time.time())
    if state is None:
        await save_day_state(store, date, DayIngestState(
            content_hash=meta.content_hash,
            revision=meta.revision,
            event_count=meta.event_count,
            last_event_id=meta.last_event_id,
            processed_at=processed_at,
        ))
        return
    await save_day_state(store, date, DayIngestState(
        content_hash=state.content_hash,
        revision=state.revision,
        event_count=state.event_count,
        last_event_id=state.last_event_id,
        processed_at=processed_at,
        source_hash=state.source_hash,
        cursor_ts=state.cursor_ts,
        cursor_id=state.cursor_id,
    ))


def incremental_ingest_enabled() -> bool:
    return os.environ.get("ENABLE_INCREMENTAL_INGEST", "false").strip().lower() in {"true", "1", "yes"}


def is_open_day(date_value: str, today: calendar_date) -> bool:
    return calendar_date.fromisoformat(date_value) >= today


async def resolve_export(
    store: IntentStore,
    client: RoleAClient,
    date: str,
    meta: DayExportMeta,
    *,
    today: calendar_date,
    state: DayIngestState | None,
) -> DayExport:
    """Fetch a day export, using incremental pages for the open day when enabled."""

    if state is not None and state.content_hash != meta.content_hash:
        await store.delete_ingest_day_events(date)
        state = None

    if not incremental_ingest_enabled() or not is_open_day(date, today):
        return await client.fetch_export(date)

    buffered = await store.list_ingest_day_events(date)
    cursor = None
    if buffered and state is not None and state.cursor_ts is not None and state.cursor_id is not None:
        cursor = EventCursor(after_ts=state.cursor_ts, after_id=state.cursor_id)

    if not buffered:
        cursor = None

    while True:
        page = await client.fetch_events_incremental(date, cursor=cursor)
        if page.events:
            await store.upsert_ingest_day_events(date, page.events)
        if not page.has_more:
            break
        if page.next_cursor is None:
            break
        cursor = page.next_cursor

    events = await store.list_ingest_day_events(date)
    if not events and meta.event_count > 0:
        await store.delete_ingest_day_events(date)
        return await client.fetch_export(date)

    return DayExport(
        date=date,
        exported_at=int(time.time()),
        events=events,
    )


async def save_processed_state(
    store: IntentStore,
    date: str,
    meta: DayExportMeta,
    *,
    source_hash: str,
    events: list[RawEvent],
) -> None:
    cursor_ts: int | None = None
    cursor_id: str | None = None
    if events:
        last = events[-1]
        cursor_ts = last.ts
        cursor_id = last.id
    await save_day_state(store, date, DayIngestState(
        content_hash=meta.content_hash,
        revision=meta.revision,
        event_count=meta.event_count,
        last_event_id=meta.last_event_id,
        processed_at=int(time.time()),
        source_hash=source_hash,
        cursor_ts=cursor_ts,
        cursor_id=cursor_id,
    ))
    if not is_open_day(date, calendar_date.today()):
        await store.delete_ingest_day_events(date)
