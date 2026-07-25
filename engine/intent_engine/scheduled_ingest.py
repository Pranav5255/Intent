"""Feature-flagged, HTTP-only scheduled ingestion for Role B."""

from __future__ import annotations

import asyncio
import os
import time
from dataclasses import dataclass
from datetime import date as calendar_date, timedelta
from pathlib import Path
from typing import Awaitable, Callable

from intent_engine.ingest_state import (
    load_day_state,
    mark_skipped,
    resolve_export,
    save_processed_state,
    should_skip,
)
from intent_engine.labeling import LabelProvider
from intent_engine.logging import DiagnosticsLogger
from intent_engine.pipeline import run_pipeline
from intent_engine.providers import create_label_provider
from intent_engine.schemas import DayExport, PipelineResult
from intent_engine.source import RoleAClient, RoleAUnavailableError
from intent_engine.store import IntentStore


LAST_COMPLETED_DATE_KEY = "scheduled_ingest_last_completed_date"
LAST_OUTCOME_KEY = "scheduled_ingest_last_outcome"
OUTCOME_SUCCESS = "success"
OUTCOME_UNCHANGED = "unchanged"
OUTCOME_ROLE_A_UNAVAILABLE = "role_a_unavailable"
OUTCOME_PIPELINE_ERROR = "pipeline_error"

PipelineRunner = Callable[[DayExport, IntentStore, LabelProvider], Awaitable[PipelineResult]]


@dataclass(frozen=True)
class ScheduledIngestResult:
    enabled: bool
    outcome: str | None
    processed_dates: tuple[str, ...] = ()
    skipped_dates: tuple[str, ...] = ()


def pipeline_runner(export: DayExport, store: IntentStore, label_provider: LabelProvider) -> Awaitable[PipelineResult]:
    """Keep the scheduled path pinned to the established non-forced pipeline call."""

    return run_pipeline(export, store, label_provider, force=False)


async def run_scheduled_ingest(
    *,
    store: IntentStore,
    role_a_client: RoleAClient,
    label_provider: LabelProvider,
    today: calendar_date | None = None,
    logger: DiagnosticsLogger | None = None,
    enabled: bool | None = None,
    run: PipelineRunner = pipeline_runner,
) -> ScheduledIngestResult:
    """Fetch Role A exports and run the unchanged Role B pipeline per date."""

    if enabled is None:
        enabled = _enabled()
    if not enabled:
        return ScheduledIngestResult(enabled=False, outcome=None)

    target_today = today or calendar_date.today()
    dates = await _dates_to_process(store, target_today)
    processed: list[str] = []
    skipped: list[str] = []
    last_outcome = OUTCOME_SUCCESS

    for date_value in dates:
        started_at = time.monotonic()
        try:
            meta = await role_a_client.fetch_export_meta(date_value)
        except RoleAUnavailableError as exc:
            if "metadata endpoint" not in str(exc).lower():
                await store.set_metadata(LAST_OUTCOME_KEY, OUTCOME_ROLE_A_UNAVAILABLE)
                _log_failure(logger, date_value, OUTCOME_ROLE_A_UNAVAILABLE, started_at)
                return ScheduledIngestResult(True, OUTCOME_ROLE_A_UNAVAILABLE, tuple(processed), tuple(skipped))
            export = await role_a_client.fetch_export(date_value)
            try:
                result = await run(export, store, label_provider)
            except Exception:
                await store.set_metadata(LAST_OUTCOME_KEY, OUTCOME_PIPELINE_ERROR)
                _log_failure(logger, date_value, OUTCOME_PIPELINE_ERROR, started_at, event_count=len(export.events))
                return ScheduledIngestResult(True, OUTCOME_PIPELINE_ERROR, tuple(processed), tuple(skipped))
            await store.set_metadata_values({
                LAST_COMPLETED_DATE_KEY: date_value,
                LAST_OUTCOME_KEY: OUTCOME_SUCCESS,
            })
            if logger:
                logger.log_scheduled_ingest(
                    date=date_value,
                    event_count=len(export.events),
                    intent_count=len(result.intents),
                    cached=result.cached,
                    duration_ms=_duration_ms(started_at),
                    outcome=OUTCOME_SUCCESS,
                )
            processed.append(date_value)
            last_outcome = OUTCOME_SUCCESS
            continue

        state = await load_day_state(store, date_value)
        if should_skip(meta, state):
            await mark_skipped(store, date_value, meta, state)
            if logger:
                logger.log_scheduled_ingest(
                    date=date_value,
                    event_count=meta.event_count,
                    intent_count=0,
                    cached=True,
                    duration_ms=_duration_ms(started_at),
                    outcome=OUTCOME_UNCHANGED,
                )
            skipped.append(date_value)
            last_outcome = OUTCOME_UNCHANGED
            continue

        try:
            export = await resolve_export(
                store,
                role_a_client,
                date_value,
                meta,
                today=target_today,
                state=state,
            )
        except RoleAUnavailableError:
            await store.set_metadata(LAST_OUTCOME_KEY, OUTCOME_ROLE_A_UNAVAILABLE)
            _log_failure(logger, date_value, OUTCOME_ROLE_A_UNAVAILABLE, started_at)
            return ScheduledIngestResult(True, OUTCOME_ROLE_A_UNAVAILABLE, tuple(processed), tuple(skipped))
        except Exception:
            await store.set_metadata(LAST_OUTCOME_KEY, OUTCOME_PIPELINE_ERROR)
            _log_failure(logger, date_value, OUTCOME_PIPELINE_ERROR, started_at)
            return ScheduledIngestResult(True, OUTCOME_PIPELINE_ERROR, tuple(processed), tuple(skipped))

        try:
            result = await run(export, store, label_provider)
        except Exception:
            await store.set_metadata(LAST_OUTCOME_KEY, OUTCOME_PIPELINE_ERROR)
            _log_failure(logger, date_value, OUTCOME_PIPELINE_ERROR, started_at, event_count=len(export.events))
            return ScheduledIngestResult(True, OUTCOME_PIPELINE_ERROR, tuple(processed), tuple(skipped))

        await save_processed_state(
            store,
            date_value,
            meta,
            source_hash=result.source_hash,
            events=export.events,
        )
        await store.set_metadata_values({
            LAST_COMPLETED_DATE_KEY: date_value,
            LAST_OUTCOME_KEY: OUTCOME_SUCCESS,
        })
        if logger:
            logger.log_scheduled_ingest(
                date=date_value,
                event_count=len(export.events),
                intent_count=len(result.intents),
                cached=result.cached,
                duration_ms=_duration_ms(started_at),
                outcome=OUTCOME_SUCCESS,
            )
        processed.append(date_value)
        last_outcome = OUTCOME_SUCCESS

    if not processed and skipped:
        await store.set_metadata(LAST_OUTCOME_KEY, OUTCOME_UNCHANGED)

    return ScheduledIngestResult(True, last_outcome, tuple(processed), tuple(skipped))


async def _dates_to_process(store: IntentStore, today: calendar_date) -> list[str]:
    last_completed = _parse_date(await store.get_metadata(LAST_COMPLETED_DATE_KEY))
    start = last_completed if last_completed is not None else today - timedelta(days=1)
    dates: list[str] = []
    current = start
    while current <= today:
        dates.append(current.isoformat())
        current += timedelta(days=1)
    return dates


def _parse_date(value: str | None) -> calendar_date | None:
    if not value:
        return None
    try:
        return calendar_date.fromisoformat(value)
    except ValueError:
        return None


def _enabled() -> bool:
    return os.environ.get("ENABLE_PIPELINE_TRIGGER", "false").strip().lower() in {"true", "1", "yes"}


def _duration_ms(started_at: float) -> int:
    return int((time.monotonic() - started_at) * 1000)


def _log_failure(
    logger: DiagnosticsLogger | None,
    date_value: str,
    outcome: str,
    started_at: float,
    *,
    event_count: int = 0,
) -> None:
    if logger:
        logger.log_scheduled_ingest(
            date=date_value,
            event_count=event_count,
            intent_count=0,
            cached=False,
            duration_ms=_duration_ms(started_at),
            outcome=outcome,
        )


def main() -> int:
    """Run once for systemd; a disabled trigger is intentionally a no-op."""

    if not _enabled():
        return 0
    database_path = os.environ.get("ROLE_B_DB_PATH", "intents.db")
    logger = DiagnosticsLogger(str(Path(database_path).with_name("engine-scheduled-ingest.jsonl")))
    result = asyncio.run(
        run_scheduled_ingest(
            store=IntentStore(database_path),
            role_a_client=RoleAClient(),
            label_provider=create_label_provider(),
            logger=logger,
            enabled=True,
        )
    )
    if result.outcome in {OUTCOME_SUCCESS, OUTCOME_UNCHANGED}:
        return 0
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
