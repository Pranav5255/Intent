from __future__ import annotations

import asyncio
import json
import sys
import tempfile
from datetime import date
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parents[1]))

from intent_engine.logging import DiagnosticsLogger
from intent_engine.labeling import TemplateFallbackLabelProvider
from intent_engine.scheduled_ingest import (
    LAST_COMPLETED_DATE_KEY,
    LAST_OUTCOME_KEY,
    OUTCOME_PIPELINE_ERROR,
    OUTCOME_ROLE_A_UNAVAILABLE,
    OUTCOME_SUCCESS,
    pipeline_runner,
    run_scheduled_ingest,
)
from intent_engine.pipeline import run_pipeline
from intent_engine.schemas import DayExport, PipelineResult
from intent_engine.source import RoleAUnavailableError
from intent_engine.store import IntentStore


def run(coroutine):
    return asyncio.run(coroutine)


class FakeRoleAClient:
    def __init__(self, exports: dict[str, DayExport], unavailable_on: str | None = None) -> None:
        self.exports = exports
        self.unavailable_on = unavailable_on
        self.calls: list[str] = []

    async def fetch_export(self, date_value: str) -> DayExport:
        self.calls.append(date_value)
        if date_value == self.unavailable_on:
            raise RoleAUnavailableError("not persisted")
        return self.exports[date_value]


def export(date_value: str) -> DayExport:
    return DayExport(date=date_value, exported_at=1, events=[])


async def successful_pipeline(export_value, _store, _provider):
    return PipelineResult(source_hash=export_value.date, pipeline_version="test", cached=False)


def test_disabled_is_a_noop() -> None:
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        client = FakeRoleAClient({})
        result = run(run_scheduled_ingest(
            store=store, role_a_client=client, label_provider=None, enabled=False, today=date(2026, 7, 14),
        ))
        assert result.enabled is False
        assert client.calls == []
        assert run(store.get_metadata(LAST_COMPLETED_DATE_KEY)) is None


def test_first_run_processes_yesterday_and_today_then_rechecks_completed_day() -> None:
    dates = ["2026-07-13", "2026-07-14"]
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        client = FakeRoleAClient({value: export(value) for value in dates})
        first = run(run_scheduled_ingest(
            store=store, role_a_client=client, label_provider=None, today=date(2026, 7, 14), enabled=True, run=successful_pipeline,
        ))
        second = run(run_scheduled_ingest(
            store=store, role_a_client=client, label_provider=None, today=date(2026, 7, 14), enabled=True, run=successful_pipeline,
        ))
        assert first.processed_dates == tuple(dates)
        assert second.processed_dates == ("2026-07-14",)
        assert client.calls == [*dates, "2026-07-14"]
        assert run(store.get_metadata(LAST_COMPLETED_DATE_KEY)) == "2026-07-14"
        assert run(store.get_metadata(LAST_OUTCOME_KEY)) == OUTCOME_SUCCESS


def test_subsequent_run_backfills_from_completed_date_through_today() -> None:
    dates = ["2026-07-11", "2026-07-12", "2026-07-13", "2026-07-14"]
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        run(store.set_metadata(LAST_COMPLETED_DATE_KEY, "2026-07-11"))
        client = FakeRoleAClient({value: export(value) for value in dates})
        result = run(run_scheduled_ingest(
            store=store, role_a_client=client, label_provider=None, today=date(2026, 7, 14), enabled=True, run=successful_pipeline,
        ))
        assert result.processed_dates == tuple(dates)
        assert client.calls == dates
        assert run(store.get_metadata(LAST_COMPLETED_DATE_KEY)) == "2026-07-14"


def test_failure_stops_batch_after_prior_success_and_logs_safe_fields() -> None:
    dates = ["2026-07-11", "2026-07-12", "2026-07-13", "2026-07-14"]
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory)
        store = IntentStore(str(path / "intents.db"))
        run(store.set_metadata(LAST_COMPLETED_DATE_KEY, "2026-07-11"))
        client = FakeRoleAClient({value: export(value) for value in dates}, unavailable_on="2026-07-13")
        logger = DiagnosticsLogger(str(path / "scheduler.jsonl"))
        result = run(run_scheduled_ingest(
            store=store, role_a_client=client, label_provider=None, today=date(2026, 7, 14), enabled=True,
            run=successful_pipeline, logger=logger,
        ))
        records = [json.loads(line) for line in (path / "scheduler.jsonl").read_text(encoding="utf-8").splitlines()]
        assert result.outcome == OUTCOME_ROLE_A_UNAVAILABLE
        assert result.processed_dates == ("2026-07-11", "2026-07-12")
        assert client.calls == ["2026-07-11", "2026-07-12", "2026-07-13"]
        assert run(store.get_metadata(LAST_COMPLETED_DATE_KEY)) == "2026-07-12"
        assert run(store.get_metadata(LAST_OUTCOME_KEY)) == OUTCOME_ROLE_A_UNAVAILABLE
        assert set(records[-1]) <= {"timestamp", "type", "date", "event_count", "intent_count", "cached", "duration_ms", "outcome"}
        assert "not persisted" not in (path / "scheduler.jsonl").read_text(encoding="utf-8")


def test_pipeline_failure_preserves_marker_and_force_is_not_available_to_runner() -> None:
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        client = FakeRoleAClient({"2026-07-13": export("2026-07-13"), "2026-07-14": export("2026-07-14")})

        async def failing_pipeline(_export, _store, _provider):
            raise RuntimeError("private payload must not persist")

        result = run(run_scheduled_ingest(
            store=store, role_a_client=client, label_provider=None, today=date(2026, 7, 14), enabled=True, run=failing_pipeline,
        ))
        assert result.outcome == OUTCOME_PIPELINE_ERROR
        assert client.calls == ["2026-07-13"]
        assert run(store.get_metadata(LAST_COMPLETED_DATE_KEY)) is None
        assert run(store.get_metadata(LAST_OUTCOME_KEY)) == OUTCOME_PIPELINE_ERROR


def test_scheduled_recheck_uses_existing_pipeline_cache() -> None:
    dates = ["2026-07-13", "2026-07-14"]
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory)
        store = IntentStore(str(path / "intents.db"))
        client = FakeRoleAClient({value: export(value) for value in dates})
        logger = DiagnosticsLogger(str(path / "scheduler.jsonl"))
        provider = TemplateFallbackLabelProvider()
        run(run_scheduled_ingest(
            store=store, role_a_client=client, label_provider=provider, today=date(2026, 7, 14), enabled=True, logger=logger,
        ))
        run(run_scheduled_ingest(
            store=store, role_a_client=client, label_provider=provider, today=date(2026, 7, 14), enabled=True, logger=logger,
        ))
        records = [json.loads(line) for line in (path / "scheduler.jsonl").read_text(encoding="utf-8").splitlines()]
        assert records[-1]["date"] == "2026-07-14"
        assert records[-1]["cached"] is True


def test_default_pipeline_wrapper_always_sets_force_false(monkeypatch) -> None:
    observed = {}

    async def recording_pipeline(export_value, store, provider, *, force=False):
        observed.update({"export": export_value, "store": store, "provider": provider, "force": force})
        return PipelineResult(source_hash="recorded", pipeline_version="test")

    monkeypatch.setattr("intent_engine.scheduled_ingest.run_pipeline", recording_pipeline)
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        export_value = export("2026-07-14")
        provider = TemplateFallbackLabelProvider()
        run(pipeline_runner(export_value, store, provider))

    assert observed["force"] is False
