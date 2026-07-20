# Role B Pipeline

## 1. Purpose

Role B turns Role A day exports or event streams into safe intent trees, bounded resume payloads, and (when explicitly enabled) grounded Copilot answers. It serves the local API on port **9478**.

Role B does not launch applications, restore windows, read Role A's SQLite database, or own the user interface. Role C/tray clients consume its API; MCP is an optional additional transport.

## 2. End-to-end flow

```text
Role A :9477                    demo-day.json
     | HTTP export/events             |
     +------------+-------------------+
                  v
       source.py (HTTP / fixture loader)
                  |
                  v
       pipeline.py (normalize -> sessions -> clusters -> enrich -> resume)
                  |
                  v
       store.py  --->  intents.db (cache, FTS, persisted intent trees)
                  |
                  v
       api.py :9478 (/intents/*, /pipeline/*, /resume/select, /copilot/*)
             /             |               \
       Role C/tray     Copilot          optional MCP
```

## 3. Deterministic layer (always on)

This layer needs no OpenAI key and is the source of truth for intent data.

### Input and normalization

| Stage | Module | Inputs / outputs | Key invariants |
|---|---|---|---|
| Source | `intent_engine/source.py` | `RoleAClient` fetches `/v1/export/day`, `/v1/events`, and `/healthz`; `load_replay_fixture()` loads a JSON `DayExport`. | Real ISO dates are required; transport failures and Role A 503s become `RoleAUnavailableError`; Pydantic validates exports/events. |
| Normalize | `intent_engine/normalize.py` | `RawEvent` -> `NormalizedEvent`; extracts family, category, safe text, entities, and signals. | Events are timestamp ordered, duplicate IDs are discarded, failures become `PipelineWarning`s, and `compute_source_hash()` returns a stable 16-character SHA-256 prefix. Raw data remains internal context only. |
| Sessions | `intent_engine/sessionize.py` | Ordered normalized events -> sessions. | A gap strictly greater than 15 minutes starts a session; `idle_start` and `idle_end` close boundaries and idle markers are not retained as activity. |
| Clusters | `intent_engine/cluster.py` | One session -> chronological topic clusters. | Five-minute adjacency, project/file/command topic shifts, and command-phase transitions form boundaries. Similar pure-gap runs may merge; output is capped at four clusters while preserving event order and uniqueness. |
| Semantic refine (optional) | `intent_engine/semantic_pack.py` + `intent_engine/semantic_cluster.py` | One session -> validated semantic clusters. | Requires `ROLE_B_SEMANTIC_CLUSTER`, `ROLE_B_LLM_ENABLED`, a selected OpenAI/Gemini key, and explicit content consent. The model receives only bounded S1 packets; invalid output, provider failures, and timeouts fall back to deterministic clusters. |

### Enrichment and persistence

| Stage | Module | Inputs / outputs | Key invariants |
|---|---|---|---|
| Enrichment | `intent_engine/enrich.py` | Cluster -> `IntentStats`, editor/browser/shell insights, project tag, TODO observations; also aggregates child stats and validates trees. | Uses normalized metadata only. Shell insights contain failed command family/exit code counts, never stderr. TODOs come only from editor `document_change` signals and store path/timestamp/marker, not code. |
| Resume | `intent_engine/resume.py` | Cluster -> `ResumePayload`; child payloads -> merged parent payload. | Most-recent editor files are capped at 5; sanitized HTTP(S) browser URLs are deduplicated by domain and capped at 8; shell keeps recent `cwd`/`last_cmd`. No restore action is performed. |
| Fallback labels | `intent_engine/labeling.py` | `TemplateFallbackLabelProvider` builds labels from cluster signals (`command_family`, file basename, domain, project tag). | Works without an API key; no Terraform/IAM keyword branches; summaries use safe normalized text only. |

When LLM labeling is disabled or fails, labels are generated from structured hints computed in `pipeline.py` (`command_family`, `top_file`, `top_domain`, `dominant_family`, `project_tag`). Examples: `Run Npm`, `Edit auth.tsx`, `Research aws.amazon.com`, `Work on infra`. Parent intents aggregate child command families or project tags into `Work on {project}` or `{fam1} and {fam2} Work`.
| Store | `intent_engine/store.py` | Persists `Intent` trees and `PipelineResult` in SQLite; provides cache, FTS search/highlighting, date/project stats, and deletion. | Writes replace a date atomically, FTS indexes only safe label/summary/insight/tag text, reads rebuild children, and `delete_date`/`delete_project` purge related rows and search entries. |
| Orchestration | `intent_engine/pipeline.py` | Runs normalization through persistence and returns `PipelineResult`. | Cache checks use normalized data plus provider identity; IDs are deterministic; parents aggregate children; `PIPELINE_VERSION` is `1.0.0`; diagnostics contain only safe aggregate fields. |

### Deterministic runtime features

- `intent_engine/current.py` (F11) fetches Role A events from the last 30 minutes, selects the latest cluster, and returns `CurrentIntent` only when confidence is at least 0.5 (`1–2` events: 0.3, `3–5`: 0.6, `>5`: 0.8). It caches qualifying results for 60 seconds.
- `intent_engine/prediction.py` (F10) indexes persisted child prefixes from the final three normalized events. The API only runs it when `ENABLE_PREDICTION=true`; insufficient history returns `null`.

### Deterministic API routes

- `GET /healthz` — service and pipeline versions.
- `GET /intents/yesterday` — roots for the previous local calendar day.
- `GET /intents?date=YYYY-MM-DD` — roots for one day.
- `GET /intents/search` — deterministic FTS/LIKE search with highlighting and optional date bounds; it never requires Copilot.
- `GET /intents/stats` — deterministic date/project aggregates.
- `GET /intents/current` — F11 current-work inference.
- `GET /intents/prediction` — feature-gated F10 prediction.
- `POST /pipeline/run?date=...` — fetch Role A export and persist a normal run.
- `POST /pipeline/run-replay` — process a supplied `DayExport` without Role A.
- `POST /pipeline/recompute?date=...` — delete the date cache and force recomputation.
- `DELETE /v1/memory/date/{date}` and `DELETE /v1/memory/project/{project}` — user-controlled durable-memory deletion.

### Deterministic privacy rules

- `POST /resume/select` resolves only stored intents into ranked candidates or one scoped preview; it never restores applications.

- No LLM provider API key is required for the deterministic pipeline.
- Clustering and resume construction do not require raw document text.
- Only bounded `ResumePayload` fields are restore context: files, URLs, and shell values.
- Raw event objects are internal pipeline context and are not API intent output or diagnostics content.
- Semantic packets exclude messaging entirely, keep Spotify/media as background-only, and include text only with explicit consent: non-redacted editor changes and non-sensitive browser excerpts are bounded before cloud transmission. Prompts, packets, raw events, titles, URLs, commands, and private/redacted content are never persisted.
- Semantic metadata contains only refinement confidence, role counts, one workspace root, and provider identity. It never contains prompts, snippets, raw events, URLs, commands, or private/redacted content.

## 4. LLM / Copilot layer (optional)

This layer is active only when `ROLE_B_LLM_ENABLED=true`, `ENABLE_COPILOT=true`, and the selected provider credentials are present (`OPENAI_API_KEY`, or for Gemini either `GEMINI_API_KEY` or a service-account JSON via `GOOGLE_APPLICATION_CREDENTIALS` / `GEMINI_CREDENTIALS_PATH`, with matching `LLM_PROVIDER`). Otherwise factories select template fallback behavior and the Copilot API returns `CopilotNotConfigured` with HTTP 503.

- `intent_engine/providers.py` — evaluates flags dynamically; creates `TemplateFallbackLabelProvider` or `LLMLabelProvider`, and optionally an OpenAI or Gemini client via `create_llm_client()`.
- `intent_engine/llm.py` — OpenAI Responses API adapter implementing the shared `LLMClient` protocol.
- `intent_engine/llm_gemini.py` — Gemini adapter implementing the same protocol for labeling and Copilot tool loops.
- `intent_engine/labeling.py` — optional LLM labeling through `LLMLabelProvider`; any SDK, timeout, validation, or network failure falls back to template labels.
- `intent_engine/tools.py` — safe read-only allowlist: `search_intents`, `get_intent`, `get_resume_payload`, `get_current_intent`, and `get_intent_stats`. Calls are capped and validated before reaching `IntentStore`/`CurrentIntentEngine`.
- `intent_engine/copilot.py` — bounded tool-calling loop with search rewriting, QA, briefing, and narrative modes; derives citations from tool results, tracks `evidence_status`, keeps compact conversation summaries, and enforces resume-proposal integrity.
- `api.py` — `POST /copilot/query` and `GET /copilot/briefing/{intent_id}`. Both are gated and return `CopilotNotConfigured` with 503 when unavailable.
- `mcp_server.py` — optional stdio MCP adapter exposing the same five tools through `ToolRegistry`; install it separately with `requirements-mcp.txt`.

### Hard Copilot rules

- LLM prompts contain only safe intent fields: labels, summaries, stats, insights, tags, TODO observations, and store-derived resume payloads.
- The LLM may not invent files, URLs, commands, dates, or restore state.
- `resume_proposal.resume_payload` must be copied from a successful `get_resume_payload`/store result; generated prose can supply only the briefing text.
- `GET /intents/search` remains deterministic. Natural-language query rewriting is Copilot-only.
- All Copilot/MCP tools are read-only. They cannot fetch raw events, touch files/Git, or call Role A `POST /v1/restore`.
- Semantic cache variants include provider/model plus content and clustering policy versions, preventing stale deterministic or cross-provider cache reuse.
- Semantic refinement uses only the existing OpenAI/Gemini cloud-client factory. Ollama, LM Studio, and other local runtimes are not supported.

### Resume selection and restore boundary

`POST /resume/select` ranks existing stored root intents by ID, project tag, or query and returns either candidates requiring a picker or one bounded preview. With `restore_scope="same_project"`, files and shell context outside the preview's unique workspace root are removed. Role B does not contact Role A or restore anything. Role C displays this preview, then only after a separate explicit user confirmation calls Role A `POST /v1/restore` with the unchanged preview payload and selected mode.

## 5. What Role B deliberately does not do

- Provide the Role C UI or tray experience.
- Restore applications or windows (Role A owns restore operations).
- Define or execute Role A's raw-event retention policy; coordinate with Role A's separate 30-day deletion process.
- Perform deferred post-MVP behavior such as idle-return notifications, five-minute pipes, or proactive auto-tasks.

## 6. Module map

| File | Layer | Role |
|---|---|---|
| `intent_engine/schemas.py` | both | Pydantic contracts for events, intents, API responses, Copilot requests, and payloads. |
| `intent_engine/source.py` | deterministic | Role A HTTP client and replay fixture loader. |
| `intent_engine/normalize.py` | deterministic | Stable event normalization, warnings, and source hashing. |
| `intent_engine/sessionize.py` | deterministic | Gap and idle session boundaries. |
| `intent_engine/cluster.py` | deterministic | Topic/command-phase clustering with a four-cluster cap. |
| `intent_engine/enrich.py` | deterministic | Stats, insights, tags, TODOs, and tree invariants. |
| `intent_engine/resume.py` | deterministic | Bounded restore-context payload construction and merging. |
| `intent_engine/labeling.py` | both | Template fallback labels and optional LLM labels (OpenAI or Gemini). |
| `intent_engine/store.py` | deterministic | SQLite persistence, cache, FTS, aggregation, and forgetting. |
| `intent_engine/pipeline.py` | both | End-to-end intent construction, caching, labeling, and persistence. |
| `intent_engine/current.py` | deterministic | F11 current-work inference. |
| `intent_engine/prediction.py` | deterministic | F10 historical prefix prediction. |
| `intent_engine/providers.py` | optional LLM | Environment-driven provider selection. |
| `intent_engine/llm_base.py` | optional LLM | Shared `LLMClient` protocol for provider adapters. |
| `intent_engine/llm.py` | optional LLM | OpenAI Responses adapter. |
| `intent_engine/llm_gemini.py` | optional LLM | Gemini adapter with tool-call continuation. |
| `intent_engine/tools.py` | optional LLM | Validated, privacy-safe Copilot tool registry. |
| `intent_engine/copilot.py` | optional LLM | Grounded Copilot tool loop and answer modes. |
| `intent_engine/api.py` | both | FastAPI routes for deterministic services and gated Copilot features. |
| `mcp_server.py` | optional LLM | Optional MCP transport for the same read-only tool registry. |
