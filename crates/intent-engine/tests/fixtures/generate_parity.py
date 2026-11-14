"""Regenerate `*.parity.json` from the 1.x Python engine.

Usage (needs pydantic>=2.9, aiosqlite, httpx, python-dotenv):

    python crates/intent-engine/tests/fixtures/generate_parity.py

Runs `engine/intent_engine` with the template label provider and semantic
clustering disabled, then keeps only the structural fields the Rust port must
reproduce. Intentional 2.0 divergences are patched in *here*, so the expected
files describe "1.x algorithm + documented fixes":

* COMMAND_FAMILIES uses the 2.0 single source of truth (adds cargo, go, pnpm,
  yarn, kubectl).
* TODO markers are not compared (1.x read them from the event headline, which
  never contains the marker; 2.0 reads the inserted text).

Summaries, ids, evidence and semantic metadata are not compared here; the Rust
snapshot files (`*.rust.json`) pin those.
"""

import asyncio
import json
import os
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[3]
sys.path.insert(0, str(REPO / "engine"))
os.environ.pop("ENGINE_SEMANTIC_CLUSTER", None)

from intent_engine import labeling, normalize  # noqa: E402
from intent_engine.labeling import TemplateFallbackLabelProvider  # noqa: E402
from intent_engine.pipeline import run_pipeline  # noqa: E402
from intent_engine.schemas import DayExport  # noqa: E402
from intent_engine.store import IntentStore  # noqa: E402

FAMILIES = ("terraform", "git", "pytest", "python", "npm", "pip", "docker", "make", "cargo", "go", "pnpm", "yarn", "kubectl")
normalize._COMMAND_FAMILIES = {name: name for name in FAMILIES}
labeling._SAFE_COMMAND_FAMILIES = frozenset(FAMILIES)

FIXTURES = ["sample-day", "coding-research-day", "debugging-day", "interrupted-day"]


def project(intent: dict) -> dict:
    return {
        "label": intent["label"],
        "confidence": intent["confidence"],
        "depth": intent["depth"],
        "start_ts": intent["start_ts"],
        "end_ts": intent["end_ts"],
        "event_count": intent["stats"]["event_count"],
        "duration_seconds": intent["stats"]["duration_seconds"],
        "sources": dict(sorted(intent["stats"]["sources"].items())),
        "tags": intent["tags"],
        "prefix": intent["prefix"],
        "insights": intent["insights"],
        "todos": [{"path": t["path"], "observed_ts": t["observed_ts"]} for t in intent["todos"]],
        "resume_payload": {
            "files": intent["resume_payload"]["files"],
            "urls": intent["resume_payload"]["urls"],
            "shell": intent["resume_payload"]["shell"],
        },
        "children": [project(child) for child in intent["children"]],
    }


async def run(name: str) -> None:
    export = DayExport.model_validate(json.loads((HERE / f"{name}.json").read_text()))
    with tempfile.TemporaryDirectory() as tmp:
        store = IntentStore(os.path.join(tmp, "intents.db"))
        if hasattr(store, "initialize"):
            await store.initialize()
        result = await run_pipeline(export, store, TemplateFallbackLabelProvider(), force=True)
    out = {"date": export.date, "intents": [project(i.model_dump(mode="json")) for i in result.intents]}
    (HERE / f"{name}.parity.json").write_text(json.dumps(out, indent=1, sort_keys=True) + "\n")
    print(name, len(result.intents), "roots")


async def main() -> None:
    for name in FIXTURES:
        await run(name)


asyncio.run(main())
