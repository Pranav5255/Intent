# Role B operator guide

## 1. What Role B is

Role B is the local Intent OS service that converts Role A activity exports into deterministic intent trees, bounded resume payloads, and optional grounded Copilot answers. The API listens on `127.0.0.1:9478`; Role A listens on `127.0.0.1:9477`. Role B does not own the UI, launch applications, restore windows, or read Role A's SQLite database. See the [pipeline guide](docs/PIPELINE.md), the [Copilot execution plan](../ROLE-B-COPILOT-EXECUTION-PLAN.md), and the [Role C handoff](../ROLE-C-HANDOFF.md) when that repository-level handoff is available.

## 2. Prerequisites

- Windows PowerShell and Python 3.11 or newer.
- Optional: Role A running on port `9477` for live exports and current-intent inference.
- Optional: a Gemini or OpenAI API key for Intent Copilot and future semantic clustering.

## 3. Setup

Run these commands from the repository root, or start in `role-b` and omit the first line:

```powershell
cd role-b
python -m venv .venv
.\.venv\Scripts\pip install -r requirements.txt
# optional:
.\.venv\Scripts\pip install -r requirements-openai.txt
.\.venv\Scripts\pip install -r requirements-mcp.txt
copy .env.example .env   # then edit secrets locally — never commit .env
```

The deterministic service works without `.env`, an LLM key, or the optional packages. Keep the real `.env` local; `.env.example` is the shareable template.

Optional provider packages:

```powershell
.\.venv\Scripts\pip install -r requirements-openai.txt
.\.venv\Scripts\pip install -r requirements-gemini.txt
```

## 4. LLM providers (optional)

Role B supports two LLM backends selected with `LLM_PROVIDER`:

| Provider | Credentials | Default model | Install |
|---|---|---|---|
| `openai` (default) | `OPENAI_API_KEY` | `gpt-4o-mini` | `requirements-openai.txt` |
| `gemini` | Service-account JSON via `GOOGLE_APPLICATION_CREDENTIALS` (preferred) or `GEMINI_API_KEY` | `gemini-2.5-flash` | `requirements-gemini.txt` |

Example Gemini `.env` using a local service-account JSON (Vertex AI):

```dotenv
LLM_PROVIDER=gemini
GOOGLE_APPLICATION_CREDENTIALS=./kube-orch-afd05706a10f.json
GOOGLE_CLOUD_PROJECT=kube-orch
GOOGLE_CLOUD_LOCATION=us-central1
ROLE_B_LLM_ENABLED=true
ENABLE_COPILOT=true
INTENT_OS_LLM_MODEL=gemini-2.5-flash
```

Keep the JSON file local — `role-b/.gitignore` ignores `kube-orch*.json` and other credential filename patterns. Never commit `.env` or service-account keys.

When `ROLE_B_LLM_ENABLED=false` or the selected provider credentials are missing, labeling uses deterministic template labels derived from cluster signals (command family, file, domain, project tag) — not keyword heuristics tied to a demo scenario.

## 4.1 Semantic correlation (optional)

Semantic refinement is disabled by default and reuses the existing cloud-provider factory. Gemini is the preferred configuration: set `LLM_PROVIDER=gemini` with a service-account JSON (or `GEMINI_API_KEY`); OpenAI remains supported through `LLM_PROVIDER=openai` and `OPENAI_API_KEY`.

```dotenv
LLM_PROVIDER=gemini
GOOGLE_APPLICATION_CREDENTIALS=./kube-orch-afd05706a10f.json
GOOGLE_CLOUD_PROJECT=kube-orch
ROLE_B_LLM_ENABLED=true
ROLE_B_SEMANTIC_CLUSTER=true
ROLE_B_SEMANTIC_TIMEOUT_MS=8000
ROLE_B_SEMANTIC_CONTENT_CONSENT=true
```

`ROLE_B_SEMANTIC_CONTENT_CONSENT=true` is mandatory. When enabled, the semantic stage sends only bounded candidate packets to the selected cloud provider: WhatsApp/messaging activity is excluded, media remains background-only, and opt-in snippets are limited to non-redacted editor changes and safe browser excerpts. The timeout defaults to `8000` milliseconds when missing or invalid.

Semantic output is advisory. Invalid proposals, low-confidence links, timeouts, provider errors, and cross-workspace merge attempts fall back to deterministic clusters. Packets, prompts, raw events, and private/redacted content are never persisted. Only the existing `openai` and `gemini` cloud factories are supported; Ollama, LM Studio, and other local LLM runtimes are out of scope.

## 5. Run the API

From `role-b`:

```powershell
.\.venv\Scripts\python -m uvicorn intent_engine.api:app --host 127.0.0.1 --port 9478
```

In another PowerShell window, check health:

```powershell
Invoke-RestMethod http://127.0.0.1:9478/healthz
```

Expected response includes `ok: true` and `pipeline_version: "1.0.0"`.

## 6. Run without Role A (replay demo)

The bundled fixture can be replayed without a live Role A process:

```powershell
$body = Get-Content tests/fixtures/demo-day.json -Raw
Invoke-RestMethod -Method Post -Uri http://127.0.0.1:9478/pipeline/run-replay -ContentType "application/json" -Body $body
```

Then read the stored intents:

```powershell
Invoke-RestMethod "http://127.0.0.1:9478/intents?date=2026-07-13"
```

For a complete diagnostic progression, run:

```powershell
.\.venv\Scripts\python tests/demo_pipeline.py
```

The diagnostic loads, normalizes, sessionizes, clusters, enriches, and validates the fixture.

## 7. Run with Role A

Start Role A on `127.0.0.1:9477` first. Then run a day export through Role B:

```powershell
Invoke-RestMethod -Method Post -Uri "http://127.0.0.1:9478/pipeline/run?date=YYYY-MM-DD"
```

Useful live reads:

```powershell
Invoke-RestMethod http://127.0.0.1:9478/intents/yesterday
Invoke-RestMethod http://127.0.0.1:9478/intents/current
```

Role A unavailability is reported as HTTP 503 by the pipeline endpoint. The current-intent endpoint returns `null` when Role A is unavailable, no recent work exists, or confidence is below the F11 threshold.

## 8. Enable Intent Copilot (optional)

Edit the local `.env` with OpenAI or Gemini credentials. For Gemini, prefer a gitignored service-account JSON:

```dotenv
LLM_PROVIDER=gemini
GOOGLE_APPLICATION_CREDENTIALS=./kube-orch-afd05706a10f.json
GOOGLE_CLOUD_PROJECT=kube-orch
GOOGLE_CLOUD_LOCATION=us-central1
ROLE_B_LLM_ENABLED=true
ENABLE_COPILOT=true
INTENT_OS_LLM_MODEL=gemini-2.5-flash
```

Restart the API after changing environment variables. Copilot is disabled unless both feature flags and provider credentials are present. When disabled, `POST /copilot/query` and the briefing helper return HTTP 503 with `code: "copilot_not_configured"`.

Example requests:

```powershell
# General grounded mode
Invoke-RestMethod -Method Post -Uri http://127.0.0.1:9478/copilot/query -ContentType "application/json" -Body '{"question":"What did I work on?","mode":"auto"}'

# Search-focused mode
Invoke-RestMethod -Method Post -Uri http://127.0.0.1:9478/copilot/query -ContentType "application/json" -Body '{"question":"Find IAM and Terraform work","mode":"search"}'

# Historical Q&A with a date range
Invoke-RestMethod -Method Post -Uri http://127.0.0.1:9478/copilot/query -ContentType "application/json" -Body '{"question":"What was I trying to fix?","mode":"qa","date_from":"2026-07-13","date_to":"2026-07-13"}'

# Briefing for one stored intent
Invoke-RestMethod -Method Post -Uri http://127.0.0.1:9478/copilot/query -ContentType "application/json" -Body '{"question":"Summarize this intent for resume","mode":"briefing","intent_id":"YOUR_INTENT_ID"}'

# Date-bounded narrative
Invoke-RestMethod -Method Post -Uri http://127.0.0.1:9478/copilot/query -ContentType "application/json" -Body '{"question":"Summarize my week","mode":"narrative","date_from":"2026-07-13","date_to":"2026-07-19"}'

# Convenience briefing route
Invoke-RestMethod http://127.0.0.1:9478/copilot/briefing/YOUR_INTENT_ID
```

Copilot answers are tool-grounded. A missing or insufficient evidence set produces an explicit insufficient-evidence response rather than invented context.

## 9. Tests

Run the suite from `role-b` with Copilot disabled (no key, or `ROLE_B_LLM_ENABLED=false` and `ENABLE_COPILOT=false`):

```powershell
.\.venv\Scripts\python -m pytest tests -v
```

The deterministic regression lock requires all tests to pass, including source, normalization, sessionization, clustering, enrichment, resume, store, pipeline, labeling, API, current-intent, prediction, Copilot fallback, and MCP-optional tests. `tests/demo_pipeline.py` must remain runnable against `tests/fixtures/demo-day.json`.

## 10. Key HTTP surface

| Method and path | Purpose | Gate/notes |
|---|---|---|
| `GET /healthz` | Service and pipeline health. | Always available. |
| `GET /intents/yesterday` | Read previous local calendar day's roots. | Empty list when no data. |
| `GET /intents?date=YYYY-MM-DD` | Read roots for a date. | Date is validated. |
| `GET /intents/search?q=&date_from=&date_to=` | Search labels/summaries/insights/tags with highlighting. | Deterministic; no LLM required. |
| `GET /intents/stats?date_from=&date_to=&project=` | Aggregate intent counts, durations, labels, and projects. | Deterministic. |
| `GET /intents/current` | Infer recent work from Role A's last 30 minutes. | F11; may return `null`. |
| `GET /intents/prediction` | Predict from historical prefixes. | Only active when `ENABLE_PREDICTION=true`. |
| `GET /intents/{intent_id}` | Read one intent tree/node. | 404 when absent. |
| `POST /resume/select` | Resolve stored intents and return a bounded preview. | Never restores; accepts `intent_id`, `project_tag`, or `query`, plus optional `restore_scope: "same_project"`. |
| `POST /pipeline/run?date=...` | Fetch Role A export and process it. | Role A required; 503 if unavailable. |
| `POST /pipeline/run-replay` | Process a `DayExport` request body. | No Role A required. |
| `POST /pipeline/recompute?date=...` | Delete and force-recompute a date. | Role A required. |
| `DELETE /v1/memory/date/{date}` | Forget all persisted intents for a date. | Role B data only. |
| `DELETE /v1/memory/project/{project}` | Forget project-tagged persisted intents. | Role B data only. |
| `POST /copilot/query` | Grounded Copilot search, QA, briefing, or narrative. | Requires Copilot flags/key; otherwise 503. |
| `GET /copilot/briefing/{intent_id}` | Convenience briefing for one intent. | Same Copilot gate. |

## Optional MCP adapter

MCP is not needed for the API. To expose the same read-only tools over stdio:

```powershell
.\.venv\Scripts\pip install -r requirements-mcp.txt
.\.venv\Scripts\python mcp_server.py
```

The adapter exposes `search_intents`, `get_intent`, `get_resume_payload`, `get_current_intent`, and `get_intent_stats` through `ToolRegistry`. It cannot restore apps, fetch raw events, access files/Git, or issue direct SQLite queries.

## Retention and forgetting

Role B retains durable intent summaries until explicitly deleted. The two `/v1/memory/...` endpoints purge Role B intent rows, cache metadata, and FTS entries. Role A's separate 30-day raw-event deletion remains a separate responsibility and is not implemented here.

## 11. Privacy and safety reminders

- Semantic refinement sends only the explicitly consented, bounded packet snippets described above; raw event objects, messaging content, titles, URLs, commands, and redacted content are never sent.
- Resume payloads are deterministic, bounded, and store-derived; generated prose cannot change their files, URLs, or shell values.
- Copilot tools are read-only and cannot call Role A restore endpoints.
- Forgetting endpoints purge Role B's durable intents and search index only; they do not delete Role A raw events.
- Never commit `.env` or API keys. Use `.env.example` as the committed template.
