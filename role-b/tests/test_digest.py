from __future__ import annotations

import asyncio
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parents[1]))

from intent_engine.digest import build_digest
from intent_engine.pipeline import run_pipeline
from intent_engine.source import load_replay_fixture
from intent_engine.store import IntentStore


def test_digest_matches_login_fixture() -> None:
    export = load_replay_fixture(str(Path(__file__).parent / "fixtures" / "demo-day.json"))
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        asyncio.run(run_pipeline(export, store))
        digest = build_digest(asyncio.run(store.get_intents_by_date(export.date)), export.date)

    assert digest["date"] == "2026-07-13"
    assert digest["headline"] == "Building Login Feature"
    assert digest["intent_count"] >= 1
    assert digest["top_intent_ids"]
    assert "auth.tsx" in digest["summary"].lower() or "edit auth.tsx" in digest["summary"].lower()
    assert "npm" in digest["summary"].lower() or "failing" in digest["summary"].lower()
