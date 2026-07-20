# Role B handover: Screenpipe-derived improvements

## Purpose and guardrails

This is an implementation handover for improving the deterministic Role B
intelligence engine using patterns observed in the screenpipe-main codebase.
It is deliberately a code-level comparison; Screenpipe documentation was not
used to derive the recommendations.

The goal is a live, explainable intent engine over Role A's approved structured
events. It is not a plan to import Screenpipe's capture product into Intent OS.

The non-negotiable privacy rule is:

> No multimodal content, raw event evidence, document text, image/OCR data, or
> audio/transcript data is sent to an LLM by this work.

Role B should work with a deterministic ruleset by default. If an optional
future LLM labeler remains supported, it must receive only an explicitly
allow-listed aggregate feature object, behind a separate opt-in. It must never
receive NormalizedEvent.evidence, NormalizedEvent.raw, document-change text,
file excerpts, browser URLs, or page/title text.

## Scope

In scope:

- Incremental, idempotent processing of Role A events into deterministic
  workflow signals and current-intent updates.
- Stronger provenance, privacy boundaries, retrieval, retention, and health
  reporting for Role B's local SQLite store.
- A migration path that preserves the existing day-export/replay pipeline.

Out of scope:

- Screen, OCR, audio, accessibility-tree, clipboard, browser-cookie, or other
  multimodal capture.
- Screenpipe's cloud workflow classifier and its cloud request path.
- Generic agent/plugin execution, autonomous actions, or cloud sync.
- Replacing Role A's consent/redaction boundary.

## Current Role B baseline

| Area | Current implementation | Consequence |
| --- | --- | --- |
| Batch pipeline | role-b/intent_engine/pipeline.py:run_pipeline normalizes a complete DayExport, sessions/clusters it, and persists a date atomically. | Good replayability, but no durable incremental cursor for live processing. |
| Current intent | role-b/intent_engine/current.py:CurrentIntentEngine rereads a rolling 30-minute window and caches it for 60 seconds. | Repeated overlapping reads can recompute the same activity and cannot make late-arrival handling explicit. |
| Provider boundary | pipeline.py and current.py build and pass `SafeIntentFeatures`; `LLMLabelProvider` revalidates that packet before every request. | Raw Role A evidence, document text, paths, URLs, titles, domains, and project identifiers do not reach optional label providers. |
| Persistence | intent_engine/store.py still stores complete Intent JSON, including evidence, but FTS now rebuilds from a safe aggregate projection. | Local raw approved context can still outlive Role A's raw-event retention policy; retention/migration remains outstanding. |
| Search | IntentStore.search_intents has FTS5 plus a LIKE fallback and an in-memory LRU cache. | It lacks query sanitisation/normalisation, relevance ordering, stable pagination, and an explicit indexed-content policy. |
| Forgetting | delete_date and delete_project delete Role B rows and FTS entries transactionally. | Useful local deletion primitives, but they are not linked to source-event lineage or a retention worker. |

The existing replay pipeline is valuable. Do not remove POST /pipeline/run-replay
or the day-based cache while adding live processing; use it as the parity oracle.

## Screenpipe patterns worth adapting

| Screenpipe code | Pattern | Role B adaptation |
| --- | --- | --- |
| crates/screenpipe-engine/src/workflow_classifier.rs | A bounded activity window, unchanged-window hash, confidence threshold, duplicate cooldown, and typed workflow event. | Keep the window, dedupe, threshold, and typed-event shape. Replace the cloud classifier with pure rules over Role A-derived features. |
| crates/screenpipe-events/src/custom_events/workflow.rs | WorkflowEvent has a kind, confidence, description, timestamp, and supporting activity entries. | Add a persisted WorkflowSignal with rule ID/version and source-event references, not raw activity strings. |
| crates/screenpipe-events/src/events_manager.rs | Decouples producers from consumers through named events. | Use a durable cursor/outbox first. An in-process notification may wake the worker, but must not be the source of truth because it is lossy on restart. |
| crates/screenpipe-db/src/text_normalizer.rs | Sanitised FTS5 terms and query expansion for compound identifiers. | Add a small tested FTS query helper for project names, paths, commands, and camelCase identifiers. |
| crates/screenpipe-db/src/db/outputs.rs | A content-hash index avoids rebuilding a search document when its source has not changed; FTS uses bm25 and explicit paging. | Add an idempotent projection/index hash and rank Intent search results deterministically. |
| crates/screenpipe-engine/src/retention.rs | Retention has configuration/status, a bounded background loop, a watermark, small time batches, and stops at a failed batch rather than skipping it. | Add a Role B retention worker for derived records and a source-lineage-aware purge path. |
| crates/screenpipe-db/src/write_queue.rs | Health separates contention from fatal DB failures, exposes last success/degraded state, and only clears a failure run after several healthy batches. | Add a smaller Role B health state around SQLite transactions and the incremental worker; do not copy Screenpipe's full Rust queue implementation. |

Do not adapt Screenpipe's classify(...) HTTP call, its classifier prompt,
EVENT_LABELS, or the OCR/audio branches in get_recent_activities. Those are
exactly the parts that would introduce external model dependence and multimodal
context.

## Target architecture

~~~text
Role A approved events
        |
        | ordered cursor: (ingested_at, id)
        v
Role B incremental worker
        |
        +--> safe feature projection
        |      (no raw evidence persistence by default)
        |
        +--> deterministic workflow rules
        |      -> WorkflowSignal + provenance
        |
        +--> incremental intent fold/current intent
        |      -> Intent projection + search document
        |
        +--> status / retention / deletion lineage
~~~

The event cursor, workflow signals, and intent projections must be committed in
one SQLite transaction. Advancing the cursor separately from writing the
derived records risks a permanent gap after a crash.

### 1. Make the provider boundary safe first

Introduce a narrow SafeIntentFeatures model and make it the only input to any
optional provider:

~~~python
class SafeIntentFeatures(BaseModel):
    project_key: str | None
    command_families: list[str]
    file_kinds: list[str]
    domains: list[str]
    event_counts: dict[str, int]
    duration_seconds: int
    boundary_reasons: list[str]
~~~

Requirements:

- In normalize.py, retain extract_evidence only for explicitly local,
  user-visible flows until a migration removes or gates it. Do not feed it into
  intelligence_text for a provider by default.
- In pipeline.py, replace the default intelligence_text provider input with a
  deterministic label built from SafeIntentFeatures.
- In current.py, replace direct label_cluster(cluster_text, ...) with the same
  deterministic label function.
- Make an LLM provider opt-in and pass it only SafeIntentFeatures; set the
  default to disabled. This is a separate product flag from any existing
  Copilot flag.
- Persist a privacy_policy_version on derived rows so old records can be
  identified and rebuilt after the migration.

Suggested deterministic labels:

- Editing <project> when editor/file signals dominate.
- Testing <project> when test command families follow edits.
- Researching <domain> when browser/domain signals dominate.
- Working in <project> as the conservative fallback.

The label function must return rule IDs and feature counts alongside the label;
confidence is a deterministic score, not a model probability.

### 2. Add a durable incremental cursor

Add these additive tables in IntentStore.init_schema:

~~~sql
CREATE TABLE role_a_cursors (
    name TEXT PRIMARY KEY,
    ingested_at INTEGER NOT NULL,
    event_id TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE workflow_signals (
    id TEXT PRIMARY KEY,
    source_event_id TEXT NOT NULL,
    source_ingested_at INTEGER NOT NULL,
    ts INTEGER NOT NULL,
    kind TEXT NOT NULL,
    confidence REAL NOT NULL,
    rule_id TEXT NOT NULL,
    ruleset_version TEXT NOT NULL,
    feature_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    UNIQUE(source_event_id, rule_id, ruleset_version)
);

CREATE INDEX idx_workflow_signals_ts ON workflow_signals(ts, id);
CREATE INDEX idx_workflow_signals_source ON workflow_signals(source_ingested_at, source_event_id);
~~~

Use (ingested_at, event_id) rather than only ts. Capture timestamps can be
late, duplicated, or supplied by a client with an incorrect clock; ingestion
order gives the worker a stable replay boundary. If Role A cannot expose this
cursor yet, document a temporary overlap window and deduplicate by event ID,
but do not call that exactly-once processing.

Role A contract needed for the durable version:

- A cursor-capable event route ordered by (ingested_at, id).
- A response containing events and next_cursor.
- Existing GET /v1/events?since= remains for compatibility and short-lived
  current-intent fallback.

Suggested Role B files:

- New intent_engine/incremental.py for polling, cursor handling, and atomic
  commits.
- intent_engine/source.py gains fetch_events_after(cursor, limit).
- intent_engine/store.py gains cursor/signal persistence helpers.
- intent_engine/api.py starts/stops the worker and exposes its status.

### 3. Implement deterministic workflow signals

Create intent_engine/workflow.py as a pure, unit-testable rules engine. It
should accept only normalised events plus safe derived features and return zero
or more signals. It must not import httpx, provider modules, or raw payload
models.

Initial rules should be deliberately small:

| Rule ID | Inputs | Signal | Confidence policy |
| --- | --- | --- | --- |
| focus.project_changed.v1 | project key changes after stable activity | project_boundary | high when both old/new projects have at least two relevant events |
| command.test_after_edit.v1 | edits followed by test/lint command | verification_phase | count edits, command success/failure, and time adjacency |
| command.deploy_after_verify.v1 | successful verification followed by deploy family | delivery_phase | high only with known command-family sequence |
| browser.research_to_editor.v1 | same project/domain research then editor activity | research_to_implementation | medium unless project correlation is explicit |
| idle.pause_resume.v1 | Role A idle boundary | work_pause or work_resume | exact, confidence 1.0 |

Borrow the useful Screenpipe mechanics from workflow_classifier.rs:

- Hash a canonical window of feature records to skip unchanged work.
- Suppress a repeated kind + project_key for a deterministic cooldown.
- Require a minimum threshold before emitting a user-facing signal.
- Keep the evidence in a compact provenance record: event IDs, rule ID,
  ruleset version, numeric feature values, and boundary reason.

Do not store raw titles, document text, page URLs, or the source activity
window in workflow_signals.

### 4. Fold signals into intents without losing replay parity

Keep run_pipeline as the batch reference implementation. The incremental path
should process a bounded tail and periodically reconcile a day:

1. Read the next page after the committed cursor.
2. Normalise and project safe features.
3. Run workflow rules and append idempotent signals.
4. Update an in-progress intent projection for the affected session/project.
5. Commit signals, projections, and cursor together.
6. On a boundary or scheduled checkpoint, run/reconcile the ordinary day
   pipeline for only the affected local time range.

This makes early live results responsive while preserving the existing batch
pipeline's reproducibility. A late event should trigger recomputation of its
small affected session/day segment, not corrupt the cursor or silently alter
an unrelated day.

CurrentIntentEngine should consume the incremental projection first. It may
fall back to the existing 30-minute Role A query when no worker state exists.

### 5. Upgrade local search safely

Create a Role B FTS helper modelled on Screenpipe's
crates/screenpipe-db/src/text_normalizer.rs:

- Quote/sanitise every user token before it reaches FTS5 MATCH.
- Add prefix handling and camelCase/number-boundary expansion for code-centric
  queries such as AuthToken, v2Api, and path-like terms.
- Use bm25(intent_search) plus a deterministic tie-breaker such as
  start_ts DESC, id ASC.
- Return a stable cursor or (score, start_ts, id) page token instead of only a
  limit. Offset pagination is acceptable only for the initial small UI.
- Index only the safe search projection: deterministic label, summary,
  project key, command families, file kinds, and allowed domains.

Do not index Intent.evidence in future search projections. The current
`safe-search-v2` index excludes it, but legacy intent JSON still needs the
retention/migration work described above.

### 6. Tie retention and deletion to provenance

Screenpipe's retention loop provides the right operational shape: explicit
configuration, status, bounded batches, a resume watermark, and no watermark
advance after a failed batch. Adapt that shape, not its media-specific modes.

Role B needs three data classes:

| Data class | Default retention | Delete behavior |
| --- | --- | --- |
| Safe workflow signals and intent projections | Configurable, longer-lived | Purge by timestamp in watermark batches |
| Raw evidence copied by legacy records | Short-lived or disabled after migration | Purge first; rebuild safe projection if required |
| Search index rows | Same lifetime as their projection | Delete in the same transaction as the owning row |

Add a role_b_retention table/configuration with enabled, days, last_cleanup,
last_error, total_deleted, and a watermark. Expose:

- GET /v1/role-b/retention/status
- POST /v1/role-b/retention/preview
- POST /v1/role-b/retention/purge with explicit confirmation

The existing Role B delete_date and delete_project should keep their
transactional FTS cleanup. Extend them to delete signals and projection rows by
source lineage. Do not claim that a Role B project delete removes Role A raw
events until Role A provides a matching, consent-safe delete contract.

### 7. Add health, recovery, and operational visibility

Model the status fields after the useful subset of Screenpipe's
WriteQueueHealth, not the full implementation:

~~~json
{
  "state": "ready | degraded | paused",
  "cursor": {"ingested_at": 0, "event_id": "..."},
  "queue_depth": 0,
  "last_success_at": 0,
  "last_error": null,
  "consecutive_failures": 0,
  "processed_events": 0,
  "duplicate_events": 0,
  "signals_emitted": 0,
  "reconciliation_backlog": 0,
  "ruleset_version": "...",
  "privacy_policy_version": "..."
}
~~~

Expose it through GET /v1/role-b/status and include no event content. Treat
SQLite lock contention separately from malformed/corrupt DB failures. Use a
bounded retry/backoff policy; after the threshold, report degraded and leave
the cursor unchanged so retry is safe. A restart must rebuild worker state from
the durable cursor rather than memory.

## Delivery order

1. **Privacy fence and data model**
   - Add SafeIntentFeatures, WorkflowSignal, provenance, and policy versions.
   - Make the deterministic labeler the default for pipeline and current
     intent.
   - Add tests proving raw evidence never reaches a provider or FTS document.

2. **Cursor and signal store**
   - Add additive migrations, atomic cursor commits, duplicate handling, and a
     replayable incremental worker.
   - Coordinate the required cursor API with Role A.

3. **Deterministic rules**
   - Implement the initial five rules, canonical-window hashing, cooldowns,
     provenance, and user-visible explanations.

4. **Projection/search migration**
   - Rebuild FTS from the safe projection, add query sanitisation/ranking, and
     retain the old search path only for a controlled migration window.

5. **Retention and health**
   - Add preview/confirmed purge, watermark batching, status, fault-injection
     tests, and rebuild/retry behavior.

6. **Parity rollout**
   - Run batch replay and incremental processing over the same fixtures.
   - Record differences by ruleset/pipeline version before switching the UI to
     incremental current-intent results.

## Required tests

- Duplicate events and a worker restart never emit duplicate signal rows.
- Reordered/late events cause only the affected local session to reconcile.
- The same fixture produces byte-stable signal/projection output for the same
  ruleset and policy versions.
- A provider spy receives only SafeIntentFeatures; it never receives an
  evidence value, raw payload, document text, URL, or file excerpt.
- FTS queries with quotes, backslashes, punctuation, camelCase, and numeric
  boundaries are safe and deterministic.
- A failed retention batch leaves its watermark unchanged and retry removes the
  same range exactly once.
- SQLite contention reports degraded without advancing the cursor; a later
  healthy transaction clears the failure streak only after the configured
  number of successes.
- delete_date/delete_project remove their workflow signals, projections, and
  index rows in one transaction.
- Existing test_pipeline.py replay/cache assertions continue to pass.

## Acceptance criteria

- Role B can derive a current intent and workflow phase from live Role A
  structured events without calling an LLM.
- Every emitted signal identifies the exact rule/ruleset and source event IDs
  that produced it.
- A crash/restart cannot skip an event page or duplicate a signal.
- Search and stored derived records contain only the approved safe projection.
- Retention, manual deletion, and status are visible and deterministic.
- Batch replay remains available and agrees with incremental output for the
  covered fixture set.

## Code references used for this handover

Intent OS:

- role-b/intent_engine/pipeline.py
- role-b/intent_engine/current.py
- role-b/intent_engine/normalize.py
- role-b/intent_engine/store.py
- role-b/intent_engine/source.py
- role-b/intent_engine/api.py

Screenpipe:

- crates/screenpipe-engine/src/workflow_classifier.rs
- crates/screenpipe-events/src/custom_events/workflow.rs
- crates/screenpipe-events/src/events_manager.rs
- crates/screenpipe-db/src/text_normalizer.rs
- crates/screenpipe-db/src/db/outputs.rs
- crates/screenpipe-engine/src/retention.rs
- crates/screenpipe-db/src/write_queue.rs
