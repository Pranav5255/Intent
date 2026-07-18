# Role B Intent Copilot Execution Plan

## Summary

Role B’s deterministic pipeline is complete enough to build on: all core stages,
SQLite/FTS storage, resume payloads, current-intent inference, and prediction
exist. It is not yet a Copilot.

This document is the architecture and ordering guide for the optional GenAI
layer. Execute the step-by-step prompts in
[`ROLE-B-COPILOT-CODEX-PROMPTS.md`](ROLE-B-COPILOT-CODEX-PROMPTS.md) one at a
time. Companion to [`ROLE-B-IMPLEMENTATION-PLAN.md`](ROLE-B-IMPLEMENTATION-PLAN.md).

The Copilot MVP is **API-only**, **read-only**, and **optional**: Role B remains
fully functional without an LLM key.

## Architecture

```text
Role A events
  → deterministic pipeline (normalize → sessionize → cluster → enrich → resume → label → store)
  → intents.db

Copilot layer (NEW, optional, needs OPENAI_API_KEY + ENABLE_COPILOT=true)
  → safe tools over Role B services
  → LLM (Responses API) decides which tools to call
  → grounded answer + citations + optional resume_proposal
```

```text
User / Role C
    │
    ▼
POST /copilot/query
    │
    ├──► Optional LLM (Responses API + function calling)
    │         │
    │         └──► SafeToolRegistry
    │                   │
    │                   ├── search_intents
    │                   ├── get_intent
    │                   ├── get_resume_payload
    │                   ├── get_current_intent
    │                   └── get_intent_stats
    │                             │
    │                             ▼
    │                      IntentStore (services only, never raw events)
    │
    └──► answer + cited intent IDs + resume_proposal (deterministic payload)

Optional MCP adapter ──► same SafeToolRegistry (identical behavior)
```

## Non-negotiable constraints

1. Keep normalization, sessionization, clustering, persistence, FTS, `resume.py`,
   F11 current intent, and prediction **deterministic**.
2. Do **not** send raw events, document contents, raw secrets, or arbitrary
   database rows to an LLM. Only privacy-safe intent fields: label, summary,
   stats, insights, tags, todos, resume_payload.
3. Keep F11 as the existing confidence-gated feature; improve wording later via
   the same safe label provider—do not rebuild it.
4. Tools call Role B **services**, never SQLite directly.
5. The model may propose a resume **target** (`intent_id` + existing payload).
   It cannot call Role A restore or invent unobserved files/URLs.
6. With `ROLE_B_LLM_ENABLED=false` (default), all existing endpoints use
   deterministic fallback; generative endpoints return a clear
   “Copilot not configured” response.

## Environment (optional LLM)

Committed: `role-b/.env.example`  
Untracked: `role-b/.env` (never commit)

```env
OPENAI_API_KEY=
ROLE_B_LLM_ENABLED=false
ENABLE_COPILOT=false
INTENT_OS_LLM_MODEL=gpt-4o-mini
ROLE_B_DB_PATH=intents.db
ENABLE_PREDICTION=false
ENABLE_PIPELINE_TRIGGER=false
```

No API key is required for the deterministic product or the existing test suite.

## Implementation order

### Phase C0 — Foundation lock + config

- Document env vars and smoke-check that the deterministic suite still passes
  with LLM disabled.
- Add Copilot request/response schemas to `intent_engine/schemas.py`.

### Phase C1 — LLM provider + factory

- Add `intent_engine/llm.py` using the OpenAI **Responses API** (not deprecated
  Chat Completions / `gpt-3.5-turbo`).
- Refactor `OpenAILabelProvider` to use the shared client.
- Add `intent_engine/providers.py` factory and wire it into `create_app()` /
  `run_pipeline()` so generative labels are selectable when enabled.

### Phase C2 — Intent-memory retrieval + forgetting

- Extend FTS search with date/time-range filters over safe fields.
- Add deterministic `get_intent_stats`.
- Add delete/forget endpoints for date and project; purge intents + FTS.
- Retention: Role B keeps durable intent summaries until the user deletes them.
  Raw activity belongs to Role A (coordinate a 30-day raw-event policy there).

### Phase C3 — Safe tool layer

MVP tools (read-only):

| Tool | Purpose |
|------|---------|
| `search_intents(query, date_from?, date_to?, limit?)` | FTS over safe intent fields |
| `get_intent(intent_id)` | Full intent tree |
| `get_resume_payload(intent_id)` | Deterministic resume payload only |
| `get_current_intent()` | F11 current intent or null |
| `get_intent_stats(date_from, date_to, project?)` | Aggregated counts/durations |

Validate IDs, dates, limits, and query sizes. Cap tool calls and returned content
per request.

### Phase C4 — Intent Copilot API

- `POST /copilot/query` — user question + optional date range.
- Model decides which safe tools to call; backend executes validated calls and
  feeds results back.
- Response: grounded answer, cited intent IDs/dates, confidence /
  insufficient-evidence status, optional `resume_proposal` with only an existing
  `intent_id` and its stored resume payload.
- Persist only a compact conversation summary + referenced intent IDs (not an
  unbounded transcript).

### Phase C5 — High-value experiences

1. **Natural-language search** — rewrite casual queries into safe FTS keywords.
2. **Intent Q&A / conversational memory** — answer only from retrieved intents.
3. **Resume briefing** — generative explanation; `resume.py` remains the only
   source of files, URLs, and shell context.
4. **Multi-day narrative** — deterministic stats first; LLM writes prose only.

### Phase C6 — MCP adapter

- Optional local MCP server exposing the **exact same** read-only tool registry.
- No second retrieval path, no raw DB access, no filesystem discovery, no restore.

### Phase C7 — Documentation handoff (no feature code)

Write three markdown docs via Codex prompts C7.1–C7.3:

| Doc | Path | Audience |
|-----|------|----------|
| Pipeline architecture (deterministic vs LLM) | `role-b/docs/PIPELINE.md` | Maintainers |
| How to run Role B | `role-b/README.md` | Anyone operating Role B |
| Role C kickoff guide | `ROLE-C-HANDOFF.md` (repo root) | Role C / UI |

Post-MVP features remain deferred (document briefly inside `PIPELINE.md` only):
constrained LLM cluster refinement, 5-minute pipes, proactive auto-tasks.

## Public interfaces

| Surface | Status |
|---------|--------|
| Existing `/intents/*`, `/pipeline/*`, F11, prediction | Unchanged, backward-compatible |
| `POST /copilot/query` | New |
| Optional MCP tools | Mirror internal registry |
| `OpenAILabelProvider` via factory | Newly selectable from `create_app()` |

## Test plan

- Preserve all existing tests with LLM disabled.
- Provider: missing key, timeout, malformed structured output, API failure →
  deterministic fallback.
- Tools: argument validation, date filtering, bounded results, no direct
  DB/raw-event access.
- Copilot (fake model): multi-tool search, grounded answer with citations,
  insufficient-evidence, no unsupported claims.
- Resume: briefing may describe the payload but cannot alter files/URLs/shell.
- MCP: each exposed tool delegates to the same internal implementation.
- Privacy: raw document text, secret-like values, unredacted URLs, and API keys
  never reach prompts, logs, or responses.
- Retention: deleted date/project disappears from FTS, retrieval, and future
  Copilot answers.

## Assumptions

- MVP is an API service for Role C (or another UI), not a new Role B chat page.
- Copilot is read-only and proposal-based; a user or Role C must approve resume.
- LLM use is optional; local configuration is required only for generative
  labels, search rewriting, Q&A, briefings, and narratives.
- MCP is an interoperability layer wrapping Role B’s service tools—not the
  database.

## How to execute

1. Read this plan once.
2. Open [`ROLE-B-COPILOT-CODEX-PROMPTS.md`](ROLE-B-COPILOT-CODEX-PROMPTS.md).
3. Run prompts in order: **C0.1 → C0.2 → … → C6.2 → C7.1 → C7.2 → C7.3**.
4. Phase **C7** is documentation only (three markdown files; no new feature modules).
5. After each code phase (C0–C6), run `pytest` from `role-b/` with LLM disabled.
