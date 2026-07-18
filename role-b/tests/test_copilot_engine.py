import asyncio
import tempfile
from pathlib import Path

from intent_engine.copilot import IntentCopilot, not_configured_response
from intent_engine.schemas import CopilotNotConfigured, CopilotQueryRequest, Intent, IntentInsights, IntentStats, PipelineResult, ResumePayload
from intent_engine.store import IntentStore
from intent_engine.tools import ToolContext, ToolRegistry


def run(awaitable):
    return asyncio.run(awaitable)


class FakeLLM:
    def __init__(self, responses):
        self.responses = list(responses)
        self.calls = []

    async def respond_with_tools(self, **kwargs):
        self.calls.append(kwargs)
        return self.responses.pop(0)


def seeded():
    directory = tempfile.TemporaryDirectory()
    store = IntentStore(str(Path(directory.name) / "intents.db"))
    intent = Intent(id="i1", date="2026-07-13", label="IAM Work", summary="Reviewed IAM policy.", start_ts=1, end_ts=2, depth=0, tags=["project:infra"], stats=IntentStats(event_count=2, duration_seconds=1), insights=IntentInsights(), resume_payload=ResumePayload(files=["/repo/iam.tf"]))
    run(store.save_pipeline_run("2026-07-13", PipelineResult(intents=[intent], source_hash="i1", pipeline_version="v1")))
    return directory, ToolRegistry(ToolContext(store))


def test_search_then_answer_has_citation():
    directory, tools = seeded()
    try:
        llm = FakeLLM([{"output_text": None, "tool_calls": [{"name": "search_intents", "arguments": {"query": "IAM"}, "call_id": "1"}]}, {"output_text": "You worked on IAM policy.", "tool_calls": []}])
        result = run(IntentCopilot(llm, tools).query(CopilotQueryRequest(question="What did I do?")))
        assert result.evidence_status == "sufficient"
        assert result.citations[0].intent_id == "i1"
        assert result.tool_calls_made == ["search_intents"]
    finally:
        directory.cleanup()


def test_search_then_get_intent_cites_id_date_and_summary():
    directory, tools = seeded()
    try:
        llm = FakeLLM([
            {"output_text": None, "tool_calls": [{"name": "search_intents", "arguments": {"query": "IAM"}, "call_id": "1"}]},
            {"output_text": None, "tool_calls": [{"name": "get_intent", "arguments": {"intent_id": "i1"}, "call_id": "2"}]},
            {"output_text": "The stored IAM work reviewed policy.", "tool_calls": []},
        ])
        result = run(IntentCopilot(llm, tools).query(CopilotQueryRequest(question="Tell me about IAM.")))
        assert result.evidence_status == "sufficient"
        assert result.citations[0].intent_id == "i1"
        assert result.citations[0].date == "2026-07-13"
        assert result.citations[0].summary == "Reviewed IAM policy."
    finally:
        directory.cleanup()


def test_natural_language_search_rewrite_merges_deduplicated_results():
    directory, tools = seeded()
    try:
        class RewriteAndAnswer(FakeLLM):
            async def respond_json(self, **kwargs):
                self.rewrite_call = kwargs
                return {"queries": ["AccessDenied", "iam", "terraform"]}

        llm = RewriteAndAnswer([{"output_text": "The stored IAM work is relevant.", "tool_calls": []}])
        result = run(IntentCopilot(llm, tools).query(CopilotQueryRequest(
            question="Why did AWS permissions fail during Terraform apply?", mode="search"
        )))
        assert result.evidence_status == "sufficient"
        assert [citation.intent_id for citation in result.citations] == ["i1"]
        assert result.tool_calls_made == ["search_intents", "search_intents", "search_intents"]
    finally:
        directory.cleanup()


def test_insufficient_and_resume_requires_tool_payload():
    directory, tools = seeded()
    try:
        empty = FakeLLM([{"output_text": "You edited /invented/file.py.", "tool_calls": []}])
        result = run(IntentCopilot(empty, tools).query(CopilotQueryRequest(question="Resume")))
        assert result.evidence_status == "insufficient"
        assert result.resume_proposal is None

        resume = FakeLLM([{"output_text": None, "tool_calls": [{"name": "get_resume_payload", "arguments": {"intent_id": "i1"}, "call_id": "1"}]}, {"output_text": "Resume the task.", "tool_calls": []}])
        result = run(IntentCopilot(resume, tools).query(CopilotQueryRequest(question="Resume i1")))
        assert result.resume_proposal is not None
        assert result.resume_proposal.resume_payload.files == ["/repo/iam.tf"]
    finally:
        directory.cleanup()


def test_not_configured_and_call_cap():
    assert isinstance(run(not_configured_response()), CopilotNotConfigured)
    directory, tools = seeded()
    try:
        tools.context.max_tool_calls = 1
        llm = FakeLLM([{"output_text": None, "tool_calls": [{"name": "search_intents", "arguments": {"query": "x"}, "call_id": "1"}]}, {"output_text": "done", "tool_calls": []}])
        result = run(IntentCopilot(llm, tools).query(CopilotQueryRequest(question="x")))
        assert result.tool_calls_made == ["search_intents"]
        assert "Tool-call limit" in result.answer
    finally:
        directory.cleanup()


def test_fake_llm_prompts_never_include_key_or_document_source():
    directory, tools = seeded()
    try:
        llm = FakeLLM([{"output_text": "No evidence.", "tool_calls": []}])
        run(IntentCopilot(llm, tools).query(CopilotQueryRequest(question="What happened?")))
        captured = str(llm.calls)
        assert "OPENAI_API_KEY" not in captured
        assert "document_change" not in captured
        assert "assume_role_policy" not in captured
    finally:
        directory.cleanup()
