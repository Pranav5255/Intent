"""Pydantic contracts shared across the Role B intent-engine pipeline."""

from __future__ import annotations

from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field, model_validator


SAFE_INTENT_PRIVACY_POLICY_VERSION = "safe-intent-features-v1"


class EventPayload(BaseModel):
    """Forward-compatible Role A event payload."""

    model_config = ConfigDict(extra="allow")


class RawEvent(BaseModel):
    """An event received from Role A's event or day-export APIs."""

    model_config = ConfigDict(extra="allow")

    id: str
    ts: int
    source: str
    type: str
    payload: EventPayload
    schema_version: int = 1


class DayExport(BaseModel):
    """Role A's complete export for one calendar day."""

    version: int = 1
    date: str
    exported_at: int
    events: list[RawEvent] = Field(default_factory=list)


class EventEntities(BaseModel):
    """Deterministically extracted facts from one normalized event."""

    project_paths: list[str] = Field(default_factory=list)
    file_path: str | None = None
    file_name: str | None = None
    file_kind: Literal["code", "pdf", "image", "other"] | None = None
    domain: str | None = None
    title: str | None = None
    command: str | None = None
    command_family: str | None = None
    cwd: str | None = None
    exit_code: int | None = None
    context_terms: list[str] = Field(default_factory=list)


class EventSignals(BaseModel):
    """Lightweight behavioral signals derived from one event."""

    typed_chars: int = 0
    save: bool = False
    todo_added: bool = False


class ContextEvidence(BaseModel):
    """One consent-approved Role A payload value available to intelligence."""

    field: str
    value: str


class NormalizedEvent(BaseModel):
    """Role B's pipeline representation, including approved Role A context."""

    id: str
    ts: int
    ordinal: int
    source: str
    family: Literal["editor", "browser", "command", "focus", "file_change", "idle", "other"]
    category: str
    text: str
    entities: EventEntities = Field(default_factory=EventEntities)
    signals: EventSignals = Field(default_factory=EventSignals)
    evidence: list[ContextEvidence] = Field(default_factory=list)
    raw: dict[str, Any] = Field(default_factory=dict)


class ResumePayload(BaseModel):
    """Context Role A can restore after the user chooses to resume an intent."""

    files: list[str] = Field(default_factory=list, max_length=5)
    urls: list[str] = Field(default_factory=list, max_length=8)
    shell: dict[str, Any] = Field(default_factory=dict)


class IntentStats(BaseModel):
    event_count: int
    duration_seconds: int
    sources: dict[str, int] = Field(default_factory=dict)
    unique_apps: list[str] = Field(default_factory=list)


class SafeIntentFeatures(BaseModel):
    """The only aggregate activity packet allowed to reach a label provider.

    This intentionally excludes evidence, event text, paths, URLs, titles, and
    project identifiers.  Those values can remain available to explicitly
    local flows, but must not cross an optional cloud-provider boundary.
    """

    model_config = ConfigDict(extra="ignore")

    policy_version: Literal["safe-intent-features-v1"] = "safe-intent-features-v1"
    project_key: str | None = None
    command_families: list[str] = Field(default_factory=list, max_length=8)
    file_kinds: list[Literal["code", "pdf", "image", "other"]] = Field(default_factory=list, max_length=4)
    domains: list[str] = Field(default_factory=list, max_length=0)
    event_counts: dict[str, int] = Field(default_factory=dict)
    duration_seconds: int = Field(default=0, ge=0, le=86_400)
    child_count: int = Field(default=0, ge=0, le=1_000)
    boundary_reasons: list[str] = Field(default_factory=list, max_length=4)


class IntentInsights(BaseModel):
    editor: list[dict[str, Any]] = Field(default_factory=list)
    browser: list[dict[str, Any]] = Field(default_factory=list)
    shell: list[dict[str, Any]] = Field(default_factory=list)


class TodoObservation(BaseModel):
    path: str
    observed_ts: int
    marker: Literal["TODO", "FIXME", "XXX"]


class SemanticIntentMetadata(BaseModel):
    """Safe provenance for an intent produced by semantic refinement."""

    refined: bool = True
    confidence: float | None = Field(default=None, ge=0.0, le=1.0)
    event_roles: dict[str, int] = Field(default_factory=dict)
    topic: str | None = Field(default=None, max_length=240)
    workspace_root: str | None = None
    provider_identity: str | None = None


class Intent(BaseModel):
    """A user-facing work intent, optionally containing sub-intents."""

    id: str
    parent_id: str | None = None
    date: str
    label: str
    summary: str
    confidence: float = Field(default=0.0, ge=0.0, le=1.0)
    start_ts: int
    end_ts: int
    depth: int
    tags: list[str] = Field(default_factory=list)
    stats: IntentStats
    insights: IntentInsights
    todos: list[TodoObservation] = Field(default_factory=list)
    evidence: list[ContextEvidence] = Field(default_factory=list)
    resume_payload: ResumePayload
    prefix: tuple[str, str, str] | None = None
    semantic: SemanticIntentMetadata | None = None
    privacy_policy_version: str = "legacy"
    children: list[Intent] = Field(default_factory=list)


class PipelineWarning(BaseModel):
    level: Literal["error", "warning"]
    message: str
    event_id: str | None = None


class PipelineResult(BaseModel):
    intents: list[Intent] = Field(default_factory=list)
    warnings: list[PipelineWarning] = Field(default_factory=list)
    source_hash: str
    pipeline_version: str
    cached: bool = False


class SemanticEventProposal(BaseModel):
    """One future LLM proposal for the role of an observed event."""

    model_config = ConfigDict(extra="forbid")

    event_id: str
    role: Literal["task", "supporting_context", "background", "unrelated"]
    confidence: float = Field(ge=0.0, le=1.0)
    topic: str = Field(min_length=1, max_length=240)
    # Groq's strict response schemas require every object property to be
    # listed in `required`.  An empty list represents no links, which is also
    # clearer than a nullable field for all callers.
    linked_event_ids: list[str]


class SemanticProposalResponse(BaseModel):
    """Structured semantic proposals returned by a future provider call."""

    model_config = ConfigDict(extra="forbid")

    proposals: list[SemanticEventProposal]


class CurrentIntent(BaseModel):
    label: str
    summary: str
    confidence: float
    since_ts: int


class PredictionResponse(BaseModel):
    predicted_label: str
    confidence: float
    resume_payload: ResumePayload


class CitedIntent(BaseModel):
    intent_id: str
    date: str
    label: str
    summary: str


class ResumeProposal(BaseModel):
    """Generative briefing paired with an unchanged store-derived resume payload."""

    intent_id: str
    resume_payload: ResumePayload
    # Briefing is generative text only; resume_payload comes from store/resume.py.
    briefing: str | None = None


class ResumeSelectRequest(BaseModel):
    """Stored-intent selection request; this never performs restoration."""

    intent_id: str | None = Field(default=None, min_length=1, max_length=128)
    project_tag: str | None = Field(default=None, min_length=1, max_length=128)
    query: str | None = Field(default=None, min_length=1, max_length=2000)
    restore_scope: Literal["same_project"] | None = None

    @model_validator(mode="after")
    def requires_selector(self) -> "ResumeSelectRequest":
        if not any(value and value.strip() for value in (self.intent_id, self.project_tag, self.query)):
            raise ValueError("one of intent_id, project_tag, or query is required")
        return self


class ResumeSelectCandidate(BaseModel):
    intent_id: str
    label: str
    summary: str
    project_tag: str | None = None
    workspace_root: str | None = None
    score: float = Field(ge=0.0)


class ResumeSelectPreview(ResumeSelectCandidate):
    resume_payload: ResumePayload


class ResumeSelectResponse(BaseModel):
    needs_picker: bool
    candidates: list[ResumeSelectCandidate] = Field(default_factory=list)
    selected: ResumeSelectPreview | None = None


class CopilotQueryRequest(BaseModel):
    question: str = Field(min_length=1, max_length=2000)
    date_from: str | None = None
    date_to: str | None = None
    mode: Literal["auto", "search", "qa", "briefing", "narrative"] = "auto"
    intent_id: str | None = None
    conversation_id: str | None = None


class CopilotQueryResponse(BaseModel):
    answer: str
    citations: list[CitedIntent] = Field(default_factory=list)
    confidence: float = Field(ge=0.0, le=1.0)
    evidence_status: Literal["sufficient", "insufficient"]
    resume_proposal: ResumeProposal | None = None
    tool_calls_made: list[str] = Field(default_factory=list)
    conversation_id: str | None = None
    cached_summary: str | None = None


class CopilotNotConfigured(BaseModel):
    ok: bool = False
    code: Literal["copilot_not_configured"] = "copilot_not_configured"
    message: str = (
        "Intent Copilot is not configured. Set ENABLE_COPILOT=true, "
        "ROLE_B_LLM_ENABLED=true, and provider credentials "
        "(OPENAI_API_KEY, GEMINI_API_KEY, or GOOGLE_APPLICATION_CREDENTIALS)."
    )


Intent.model_rebuild()
