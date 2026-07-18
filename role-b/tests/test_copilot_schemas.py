import pytest
from pydantic import ValidationError

from intent_engine.schemas import (
    CitedIntent,
    CopilotNotConfigured,
    CopilotQueryRequest,
    CopilotQueryResponse,
    ResumePayload,
    ResumeProposal,
)


def test_request_response_round_trip():
    request = CopilotQueryRequest(
        question="What was I working on?",
        date_from="2026-07-13",
        mode="qa",
        conversation_id="conversation-1",
    )
    response = CopilotQueryResponse(
        answer="You worked on IAM policy changes.",
        citations=[CitedIntent(intent_id="i1", date="2026-07-13", label="IAM work", summary="Policy edits")],
        confidence=0.8,
        evidence_status="sufficient",
        conversation_id=request.conversation_id,
    )

    assert CopilotQueryRequest.model_validate(request.model_dump()) == request
    assert CopilotQueryResponse.model_validate(response.model_dump()) == response


def test_question_constraints():
    with pytest.raises(ValidationError):
        CopilotQueryRequest(question="")
    with pytest.raises(ValidationError):
        CopilotQueryRequest(question="x" * 2001)


def test_resume_proposal_preserves_resume_limits():
    payload = ResumePayload(files=[f"file-{i}" for i in range(5)], urls=[f"https://example.com/{i}" for i in range(8)])
    proposal = ResumeProposal(intent_id="i1", resume_payload=payload)
    assert proposal.resume_payload == payload

    with pytest.raises(ValidationError):
        ResumeProposal(intent_id="i1", resume_payload=ResumePayload(files=[str(i) for i in range(6)]))
    with pytest.raises(ValidationError):
        ResumeProposal(intent_id="i1", resume_payload=ResumePayload(urls=[f"https://e.test/{i}" for i in range(9)]))


def test_response_list_defaults_are_independent():
    first = CopilotQueryResponse(answer="a", confidence=0.5, evidence_status="insufficient")
    second = CopilotQueryResponse(answer="b", confidence=0.5, evidence_status="insufficient")
    first.tool_calls_made.append("search_intents")
    assert second.tool_calls_made == []
    assert first.citations == []
    assert second.citations == []


def test_not_configured_defaults():
    model = CopilotNotConfigured()
    assert model.ok is False
    assert model.code == "copilot_not_configured"
    assert "ENABLE_COPILOT=true" in model.message
