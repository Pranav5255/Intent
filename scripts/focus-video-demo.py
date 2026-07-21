#!/usr/bin/env python3
"""Focus an isolated replay on the login session used in the recording pitch.

This script is deliberately for the disposable video-demo database only. It
keeps the authentic restore payload generated from the provided Role B replay,
then narrows the presentation to its taskflow auth session and adds a local
searchable topic label. It must never run against a normal Intent store.
"""

from __future__ import annotations

import argparse
import asyncio
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "role-b"))

from intent_engine.schemas import Intent, PipelineResult
from intent_engine.store import IntentStore


PITCH_DURATION_SECONDS = 2 * 60 * 60 + 14 * 60
PITCH_SUMMARY = "Built the login feature with JWT authorization, edited auth.tsx, and debugged failing npm tests."


def _contains_auth_file(intent: Intent) -> bool:
    return any(path.endswith("/taskflow-app/src/auth.tsx") for path in intent.resume_payload.files)


def _with_tags(intent: Intent, *tags: str) -> list[str]:
    return list(dict.fromkeys([*intent.tags, *tags]))


def _focus_login_session(roots: list[Intent]) -> Intent:
    root = next((item for item in roots if _contains_auth_file(item)), None)
    if root is None:
        raise ValueError("The replay did not produce a taskflow-app auth.tsx restore session")

    children: list[Intent] = []
    for child in root.children:
        if child.resume_payload.urls:
            children.append(child.model_copy(update={
                "label": "Debug npm test",
                "summary": "Debugged failing npm tests while researching JWT authorization.",
                "tags": _with_tags(child, "topic:jwt", "topic:login"),
            }))
        else:
            children.append(child.model_copy(update={
                "label": "Edit auth.tsx",
                "summary": "Edited the login flow in auth.tsx.",
                "tags": _with_tags(child, "topic:login"),
            }))

    stats = root.stats.model_copy(update={"duration_seconds": PITCH_DURATION_SECONDS})
    return root.model_copy(update={
        "label": "Building Login Feature",
        "summary": PITCH_SUMMARY,
        "stats": stats,
        "tags": _with_tags(root, "topic:jwt", "topic:login"),
        "children": children,
    })


async def focus(database: Path, date: str) -> Intent:
    store = IntentStore(str(database))
    roots = await store.get_intents_by_date(date)
    root = _focus_login_session(roots)
    await store.save_pipeline_run(
        date,
        PipelineResult(
            intents=[root],
            source_hash="video-demo-login-focus-v1",
            pipeline_version="video-demo-focus-v1",
        ),
    )
    return root


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--database", required=True, type=Path)
    parser.add_argument("--date", required=True)
    args = parser.parse_args()
    root = asyncio.run(focus(args.database, args.date))
    print(f"Focused demo intent: {root.label} ({root.id})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
