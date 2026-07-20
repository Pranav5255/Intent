import asyncio
import tempfile
from pathlib import Path

from intent_engine.copilot import IntentCopilot
from intent_engine.schemas import CopilotQueryRequest, Intent, IntentInsights, IntentStats, PipelineResult, ResumePayload
from intent_engine.store import IntentStore
from intent_engine.tools import ToolContext, ToolRegistry


class BriefingLLM:
    def __init__(self):
        self.calls = []

    async def respond_with_tools(self, **kwargs):
        self.calls.append(kwargs)
        return {
            "output_text": "You were fixing IAM permissions for Terraform apply. Resume /invented.tf, despite the stored payload.",
            "tool_calls": [],
        }


def _seed():
    temp = tempfile.TemporaryDirectory()
    store = IntentStore(str(Path(temp.name) / "intents.db"))
    payload = ResumePayload(files=["/repo/iam.tf"], urls=["https://docs.aws.amazon.com/iam"], shell={"cwd": "/repo", "last_cmd": "terraform apply"})
    intent = Intent(
        id="brief-1", date="2026-07-13", label="IAM Terraform", summary="Fixed AccessDenied IAM policy.",
        start_ts=1, end_ts=2, depth=0, stats=IntentStats(event_count=3, duration_seconds=1),
        insights=IntentInsights(), resume_payload=payload,
    )
    asyncio.run(store.save_pipeline_run("2026-07-13", PipelineResult(intents=[intent], source_hash="brief", pipeline_version="1")))
    return temp, store, payload


def test_briefing_copies_store_payload_and_ignores_invented_fields():
    temp, store, expected = _seed()
    try:
        llm = BriefingLLM()
        result = asyncio.run(IntentCopilot(llm, ToolRegistry(ToolContext(store))).query(
            CopilotQueryRequest(mode="briefing", intent_id="brief-1", question="Summarize this intent for resume")
        ))
        assert result.evidence_status == "sufficient"
        assert result.resume_proposal is not None
        assert result.resume_proposal.briefing
        assert result.resume_proposal.resume_payload.model_dump() == expected.model_dump()
        assert "/invented.tf" not in result.resume_proposal.resume_payload.files
        captured = str(llm.calls)
        for marker in ("/repo/iam.tf", "https://docs.aws.amazon.com/iam", "terraform apply"):
            assert marker not in captured
        assert "resume_payload_available" in captured
    finally:
        temp.cleanup()
