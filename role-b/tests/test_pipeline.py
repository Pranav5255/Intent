from __future__ import annotations

import asyncio
import json
import sys
import tempfile
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parents[1]))

from intent_engine.pipeline import run_pipeline
from intent_engine.logging import DiagnosticsLogger
from intent_engine.labeling import LabelProvider
from intent_engine.schemas import DayExport, EventPayload, RawEvent
from intent_engine.source import load_replay_fixture
from intent_engine.store import IntentStore


def test_pipeline_builds_safe_deterministic_cached_intents() -> None:
    fixture = Path(__file__).parent / "fixtures" / "demo-day.json"
    export = load_replay_fixture(str(fixture))
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        first = asyncio.run(run_pipeline(export, store))
        second = asyncio.run(run_pipeline(export, store))
        forced = asyncio.run(run_pipeline(export, store, force=True))

    assert first.cached is False
    assert second.cached is True
    assert forced.cached is False
    assert first.source_hash == second.source_hash
    assert first.intents and first.intents[0].children
    assert first.intents[0].id == second.intents[0].id
    assert first.intents[0].id == forced.intents[0].id
    assert first.intents[0].label == "Work in project:infra"
    assert first.intents[0].confidence == 0.7
    assert all(child.confidence > 0 for root in first.intents for child in root.children)
    assert any(todo.path.endswith("iam.tf") and todo.marker == "TODO" for root in first.intents for child in root.children for todo in child.todos)
    serialized = json.dumps(first.model_dump(mode="json"))
    assert "raw" not in serialized
    assert "document_change" not in serialized
    assert all(child.depth == 1 for root in first.intents for child in root.children)


def test_single_cluster_becomes_root_and_optional_logger_records_safe_outcomes() -> None:
    export = DayExport(
        date="2026-07-14",
        exported_at=1,
        events=[RawEvent(id="one", ts=1, source="vscode", type="file_save", payload=EventPayload(file_path="/repo/a.py"))],
    )
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        log_path = Path(directory) / "role-b.jsonl"
        logger = DiagnosticsLogger(str(log_path))
        first = asyncio.run(run_pipeline(export, store, logger=logger))
        second = asyncio.run(run_pipeline(export, store, logger=logger))
        records = [json.loads(line) for line in log_path.read_text(encoding="utf-8").splitlines()]

    assert first.intents[0].depth == 0
    assert first.intents[0].parent_id is None
    assert first.intents[0].children == []
    assert second.cached is True
    assert [record["type"] for record in records] == ["pipeline_run", "cache_hit"]
    assert records[0]["event_count"] == 1


def test_logger_records_error_without_event_content() -> None:
    class FailingStore:
        async def get_cached_intents(self, date, source_hash):
            return None

        async def save_pipeline_run(self, date, result):
            raise RuntimeError("database failure")

    export = DayExport(
        date="2026-07-15",
        exported_at=1,
        events=[RawEvent(id="one", ts=1, source="vscode", type="file_save", payload=EventPayload())],
    )
    with tempfile.TemporaryDirectory() as directory:
        log_path = Path(directory) / "role-b.jsonl"
        with pytest.raises(RuntimeError, match="database failure"):
            asyncio.run(run_pipeline(export, FailingStore(), logger=DiagnosticsLogger(str(log_path))))
        record = json.loads(log_path.read_text(encoding="utf-8"))

    assert record["status"] == "error"
    assert record["event_count"] == 1
    assert "payload" not in record


def test_pipeline_labels_children_and_parent_with_provider_and_isolates_cache() -> None:
    class RecordingProvider(LabelProvider):
        cache_identity = "recording-v1"

        def __init__(self) -> None:
            self.cluster_texts: list[str] = []
            self.parent_texts: list[str] = []

        async def label_cluster(self, cluster_events_text: str, project_tag: str | None = None) -> dict:
            self.cluster_texts.append(cluster_events_text)
            return {"label": "Custom Child Label", "summary": "Custom child summary.", "confidence": 0.91}

        async def label_parent(self, parent_events_text: str, project_tag: str | None = None) -> dict:
            self.parent_texts.append(parent_events_text)
            return {"label": "Custom Parent Label", "summary": "Custom parent summary.", "confidence": 0.82}

    export = load_replay_fixture(str(Path(__file__).parent / "fixtures" / "demo-day.json"))
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        fallback = asyncio.run(run_pipeline(export, store))
        provider = RecordingProvider()
        result = asyncio.run(run_pipeline(export, store, provider))
        cached = asyncio.run(run_pipeline(export, store, RecordingProvider()))

    assert fallback.source_hash != result.source_hash
    assert result.cached is False
    assert cached.cached is True
    assert result.intents[0].label == "Custom Parent Label"
    assert result.intents[0].confidence == 0.82
    assert all(child.label == "Custom Child Label" and child.confidence == 0.91 for child in result.intents[0].children)
    assert provider.cluster_texts[0].startswith("1. ")
    assert "raw" not in provider.cluster_texts[0].lower()
    assert provider.parent_texts[0].startswith("1. Custom Child Label: Custom child summary.")
