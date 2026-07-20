import pytest
from pydantic import ValidationError

from intent_engine.schemas import SemanticEventProposal, SemanticProposalResponse


def test_semantic_proposals_accept_valid_roles_and_links():
    response = SemanticProposalResponse(
        proposals=[
            SemanticEventProposal(
                event_id="event-1",
                role="supporting_context",
                confidence=0.75,
                topic="Type-safe authentication",
                linked_event_ids=["event-2", "event-3"],
            )
        ]
    )

    assert response.proposals[0].linked_event_ids == ["event-2", "event-3"]


@pytest.mark.parametrize("role", ["", "related", "task_like"])
def test_semantic_proposals_reject_invalid_roles(role):
    with pytest.raises(ValidationError):
        SemanticEventProposal(event_id="event-1", role=role, confidence=0.5, topic="unknown", linked_event_ids=[])


@pytest.mark.parametrize("confidence", [-0.01, 1.01])
def test_semantic_proposals_reject_out_of_range_confidence(confidence):
    with pytest.raises(ValidationError):
        SemanticEventProposal(event_id="event-1", role="task", confidence=confidence, topic="unknown", linked_event_ids=[])


def test_semantic_proposals_require_an_explicit_empty_link_list():
    with pytest.raises(ValidationError):
        SemanticEventProposal(event_id="event-1", role="task", confidence=0.5, topic="unknown")
