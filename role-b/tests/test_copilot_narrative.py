import asyncio
import os
import tempfile
from pathlib import Path

os.environ.setdefault("ROLE_B_DB_PATH", str(Path(tempfile.gettempdir()) / "role-b-narrative-import.db"))

from fastapi.testclient import TestClient

from intent_engine.api import create_app
from intent_engine.copilot import IntentCopilot
from intent_engine.schemas import CopilotQueryRequest, Intent, IntentInsights, IntentStats, PipelineResult, ResumePayload
from intent_engine.store import IntentStore
from intent_engine.tools import ToolContext, ToolRegistry


class NarrativeLLM:
    def __init__(self):
        self.calls = []

    async def respond_with_tools(self, **kwargs):
        self.calls.append(kwargs)
        return {"output_text": "You completed 5 events across 120 seconds this week.", "tool_calls": []}


def test_narrative_uses_stats_and_preserves_numbers():
    temp = tempfile.TemporaryDirectory()
    try:
        store = IntentStore(str(Path(temp.name) / "intents.db"))
        roots = [
            Intent(id="n1", date="2026-07-13", label="IAM", summary="IAM work", start_ts=1, end_ts=61, depth=0, stats=IntentStats(event_count=3, duration_seconds=60), insights=IntentInsights(), resume_payload=ResumePayload()),
            Intent(id="n2", date="2026-07-14", label="Terraform", summary="Terraform work", start_ts=2, end_ts=62, depth=0, stats=IntentStats(event_count=2, duration_seconds=60), insights=IntentInsights(), resume_payload=ResumePayload()),
        ]
        for root in roots:
            asyncio.run(store.save_pipeline_run(root.date, PipelineResult(intents=[root], source_hash=root.id, pipeline_version="1")))
        llm = NarrativeLLM()
        result = asyncio.run(IntentCopilot(llm, ToolRegistry(ToolContext(store))).query(CopilotQueryRequest(
            mode="narrative", question="Summarize my week", date_from="2026-07-13", date_to="2026-07-14"
        )))
        assert result.evidence_status == "sufficient"
        assert "5" in result.answer and "120" in result.answer
        assert result.resume_proposal is None
        assert '"event_count":5' in str(llm.calls[0]["messages"])
    finally:
        temp.cleanup()


def test_narrative_requires_date_range(monkeypatch):
    monkeypatch.delenv("ENABLE_COPILOT", raising=False)
    monkeypatch.delenv("ROLE_B_LLM_ENABLED", raising=False)
    monkeypatch.delenv("OPENAI_API_KEY", raising=False)
    with tempfile.TemporaryDirectory() as directory:
        client = TestClient(create_app(IntentStore(str(Path(directory) / "intents.db"))))
        response = client.post("/copilot/query", json={"mode": "narrative", "question": "Summarize"})
        assert response.status_code == 400
