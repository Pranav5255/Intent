from __future__ import annotations

import asyncio
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parents[1]))

from intent_engine.context import build_intent_context
from intent_engine.pipeline import run_pipeline
from intent_engine.source import load_replay_fixture
from intent_engine.store import IntentStore


def test_context_includes_auth_and_npm_failure() -> None:
    export = load_replay_fixture(str(Path(__file__).parent / "fixtures" / "demo-day.json"))
    with tempfile.TemporaryDirectory() as directory:
        store = IntentStore(str(Path(directory) / "intents.db"))
        result = asyncio.run(run_pipeline(export, store))
        parent = result.intents[0]
        markdown = build_intent_context(parent)

    lowered = markdown.lower()
    assert "auth.tsx" in lowered
    assert "npm" in lowered
    assert "document_change" not in lowered
    assert len(markdown) <= 2048
