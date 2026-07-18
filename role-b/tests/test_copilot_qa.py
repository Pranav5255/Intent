import asyncio
import tempfile
from pathlib import Path

from intent_engine.copilot import IntentCopilot
from intent_engine.schemas import (
    CopilotQueryRequest,
    Intent,
    IntentInsights,
    IntentStats,
    PipelineResult,
    ResumePayload,
)
from intent_engine.store import IntentStore
from intent_engine.tools import ToolContext, ToolRegistry


class FakeLLM:
    def __init__(self):
        self.calls = []
        self.responses = [
            {"output_text": None, "tool_calls": [{"name": "search_intents", "arguments": {"query": "IAM"}, "call_id": "s1"}]},
            {"output_text": None, "tool_calls": [{"name": "get_intent", "arguments": {"intent_id": "qa-1"}, "call_id": "i1"}]},
            {"output_text": "Terraform apply failed with AccessDenied while editing IAM policy; see intent qa-1.", "tool_calls": []},
        ]

    async def respond_with_tools(self, **kwargs):
        self.calls.append(kwargs)
        return self.responses.pop(0)


def test_qa_is_grounded_and_propagates_dates():
    temp = tempfile.TemporaryDirectory()
    try:
        store = IntentStore(str(Path(temp.name) / "intents.db"))
        intent = Intent(
            id="qa-1",
            date="2026-07-13",
            label="Terraform IAM troubleshooting",
            summary="Fixed an AccessDenied Terraform apply by editing IAM policy.",
            start_ts=100,
            end_ts=200,
            depth=0,
            stats=IntentStats(event_count=4, duration_seconds=100),
            insights=IntentInsights(shell=[{"command_family": "terraform", "exit_code": 1, "count": 1}]),
            resume_payload=ResumePayload(),
        )
        asyncio.run(store.save_pipeline_run("2026-07-13", PipelineResult(intents=[intent], source_hash="qa" , pipeline_version="1")))
        fake = FakeLLM()
        class RecordingRegistry(ToolRegistry):
            def __init__(self, context):
                super().__init__(context)
                self.executed = []
            async def execute(self, name, arguments):
                self.executed.append((name, arguments))
                return await super().execute(name, arguments)

        registry = RecordingRegistry(ToolContext(store))
        result = asyncio.run(IntentCopilot(fake, registry).query(
            CopilotQueryRequest(
                question="What was I trying to fix yesterday afternoon?",
                mode="qa",
                date_from="2026-07-13",
                date_to="2026-07-13",
            )
        ))
        assert result.evidence_status == "sufficient"
        assert result.citations[0].intent_id == "qa-1"
        assert "Terraform" in result.answer
        assert "AccessDenied" in result.answer
        assert "retrieve evidence" in fake.calls[0]["system"]
        assert "insights.shell" in fake.calls[0]["system"]
        assert registry.executed[0][1]["date_from"] == "2026-07-13"
        assert registry.executed[0][1]["date_to"] == "2026-07-13"
    finally:
        temp.cleanup()


def test_qa_without_evidence_is_insufficient():
    temp = tempfile.TemporaryDirectory()
    try:
        store = IntentStore(str(Path(temp.name) / "intents.db"))

        class EmptyLLM:
            async def respond_with_tools(self, **kwargs):
                return {"output_text": "There is no evidence.", "tool_calls": []}

        result = asyncio.run(IntentCopilot(EmptyLLM(), ToolRegistry(ToolContext(store))).query(
            CopilotQueryRequest(question="What was I trying to fix yesterday afternoon?", mode="qa")
        ))
        assert result.evidence_status == "insufficient"
        assert result.resume_proposal is None
    finally:
        temp.cleanup()
