#!/usr/bin/env python3
"""Offline checks for LLM-facing payload quality before token optimization."""

from __future__ import annotations

import asyncio
import json
import sys
from pathlib import Path

ENGINE_ROOT = Path(__file__).resolve().parents[1]
if str(ENGINE_ROOT) not in sys.path:
    sys.path.insert(0, str(ENGINE_ROOT))

from intent_engine.labeling import (  # noqa: E402
    TemplateFallbackLabelProvider,
    build_safe_cluster_features,
    serialize_safe_features,
)
from intent_engine.normalize import normalize_events  # noqa: E402
from intent_engine.pipeline import run_pipeline  # noqa: E402
from intent_engine.schemas import DayExport, RawEvent  # noqa: E402
from intent_engine.semantic_cluster import _packet_summary  # noqa: E402
from intent_engine.semantic_pack import build_semantic_candidate_packets  # noqa: E402
from intent_engine.sessionize import sessionize  # noqa: E402
from intent_engine.store import IntentStore  # noqa: E402


def _load_fixture(path: Path) -> DayExport:
    payload = json.loads(path.read_text(encoding="utf-8"))
    events = [RawEvent.model_validate(item) for item in payload["events"]]
    return DayExport(
        version=payload.get("version", 1),
        date=payload["date"],
        exported_at=payload.get("exported_at", 0),
        events=events,
    )


def _assert_labeling_features(export: DayExport) -> None:
    normalized, _warnings = normalize_events(export.events)
    session = asyncio.run(sessionize(normalized))[0]
    features = build_safe_cluster_features(
        session,
        semantic_topic="Python asyncio timeouts",
        project_tag="project:sample-app",
    )
    assert features.domains, "expected domain roots in labeling features"
    assert features.file_names, "expected file basenames in labeling features"
    assert features.semantic_topic == "Python asyncio timeouts"
    serialized = json.loads(serialize_safe_features(features))
    assert serialized["policy_version"] == "safe-intent-features-v2"


def _assert_semantic_timeline(export: DayExport) -> None:
    normalized, _warnings = normalize_events(export.events)
    session = asyncio.run(sessionize(normalized))[0]
    packets = build_semantic_candidate_packets(session)
    assert packets, "expected semantic packets from fixture session"
    summary = _packet_summary("p0", packets[0], {})
    assert "timeline" in summary, "expected chronological timeline in semantic packet summary"
    assert len(summary["timeline"]) >= 2


async def _assert_pipeline_labels(export: DayExport) -> None:
    store = IntentStore(Path("/tmp/intent-eval-intents.db"))
    result = await run_pipeline(export, store, label_provider=TemplateFallbackLabelProvider(), force=True)
    labels = [intent.label for intent in result.intents]
    flattened = []
    for intent in result.intents:
        flattened.append(intent.label)
        flattened.extend(child.label for child in intent.children)
    assert flattened, "expected pipeline intents"
    assert not all(label in {"Work Task", "Work Session"} for label in flattened), (
        f"labels should use topical evidence, got: {flattened}"
    )


def main() -> int:
    fixture = ENGINE_ROOT / "fixtures" / "sample-day.json"
    if not fixture.is_file():
        print(f"fixture missing: {fixture}", file=sys.stderr)
        return 2
    export = _load_fixture(fixture)
    _assert_labeling_features(export)
    _assert_semantic_timeline(export)
    asyncio.run(_assert_pipeline_labels(export))
    print("eval_llm_quality: PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
