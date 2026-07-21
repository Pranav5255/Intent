# Role C integration handoff

This document is the starting point for the Role C UI engineer. Role C should integrate through HTTP contracts only; it should not reimplement Role B inference or persistence.

## 1. System split

| Role | Port | Responsibility |
|---|---:|---|
| Role A | 9477 | Capture activity events and execute application restore. |
| Role B | 9478 | Normalize/group events, persist intents, search, Copilot, and build deterministic resume payloads. |
| Role C | 9479 (or another local UI port) | UI presentation and user-confirmed actions only. |

## 2. What Role C must not do

- Do not read Role A or Role B SQLite files.
- Do not invent file paths, URLs, commands, or shell context for restore.
- Do not call Role A restore with an LLM-invented payload.
- Do not send raw editor document text to any cloud API from the UI when Role B has already redacted and aggregated it.
- Do not duplicate clustering, intent inference, prediction, or Copilot tool logic in the UI.

## 3. Bootstrap order for the local demo

1. Start Role A on `127.0.0.1:9477` when using live exports/current intent. It is optional for replay-only demos.
2. Start Role B on `127.0.0.1:9478` using the commands in [`role-b/README.md`](role-b/README.md).
3. Seed Role B with `role-b/tests/fixtures/demo-day.json` through `POST /pipeline/run-replay`, or run `POST /pipeline/run?date=YYYY-MM-DD` with Role A available.
4. Start Role C and point its API client at `http://127.0.0.1:9478`.
5. Role B currently allows browser origins `http://localhost:3000`, `http://127.0.0.1:3000`, and `http://localhost:5000`. Coordinate a Role B CORS update before using another origin/port.

## 4. Primary UI data contracts

### Intent tree

Render the fields returned by Role B's `Intent` model:

```json
{
  "id": "...",
  "parent_id": null,
  "date": "2026-07-13",
  "label": "...",
  "summary": "...",
  "confidence": 0.8,
  "start_ts": 0,
  "end_ts": 0,
  "depth": 0,
  "tags": ["project:infra"],
  "stats": {"event_count": 0, "duration_seconds": 0, "sources": {}, "unique_apps": []},
  "insights": {"editor": [], "browser": [], "shell": []},
  "todos": [],
  "resume_payload": {"files": [], "urls": [], "shell": {}},
  "prefix": null,
  "children": []
}
```

`children` contains nested child intents. `insights.shell` is aggregated metadata, not raw stderr or command output. `todos` contains only `{path, observed_ts, marker}`.

### ResumePayload

```json
{
  "files": ["/workspace/iam.tf"],
  "urls": ["https://docs.aws.amazon.com/iam"],
  "shell": {"cwd": "/workspace", "last_cmd": "terraform apply"}
}
```

Role B caps files at 5 and URLs at 8. For Firefox, each URL is the final sanitized URL observed for one tab in the selected intent; distinct tabs on the same domain remain distinct. Restore URLs are HTTP(S) only; treat the payload as authoritative and display it without expanding or inventing values.

### CurrentIntent

`GET /intents/current` returns either `null` or:

```json
{"label":"...","summary":"...","confidence":0.6,"since_ts":1720000000}
```

### CopilotQueryResponse

Copilot responses contain `answer`, `citations`, `evidence_status`, `confidence`, optional `resume_proposal`, `tool_calls_made`, `conversation_id`, and `cached_summary`. A resume proposal contains an `intent_id`, a store-derived `resume_payload`, and optional generative `briefing` text.

When Copilot is disabled, Role B returns HTTP 503 with:

```json
{
  "ok": false,
  "code": "copilot_not_configured",
  "message": "Intent Copilot is not configured..."
}
```

When a configured Copilot provider request fails, `POST /copilot/query` and
`GET /copilot/briefing/{intent_id}` return HTTP 502 with:

```json
{
  "detail": "Copilot provider request failed"
}
```

A successful HTTP 200 response with `evidence_status: "insufficient"` is not
an error: Role B found no adequate stored evidence for the question.

## 5. Recommended screens and calls

| Screen | Role B call | Notes |
|---|---|---|
| Yesterday / timeline | `GET /intents/yesterday` or `GET /intents?date=...` | Render roots and nested `children`. |
| Intent detail | `GET /intents/{id}` | Show summary, stats, insights, todos, and resume payload. |
| Search | `GET /intents/search?q=...` | Deterministic FTS/LIKE search; show `highlight_snippet`. |
| Now | `GET /intents/current` | Poll approximately every 60 seconds; handle `null`. |
| Copilot chat | `POST /copilot/query` | Optional; disable gracefully on 503. On 502, keep the panel available and offer a retry. |
| Briefing | `GET /copilot/briefing/{id}` | Show generative briefing separately from deterministic payload; offer retry on 502. |
| Resume selection | `POST /resume/select` | Resolve stored intents and return a preview only; it never restores applications. |
| Resume CTA | Use `resume_proposal.resume_payload` or `intent.resume_payload`, then call Role A | Require user confirmation and choose `resume` or `continue` mode. |

## 6. Resume flow (critical)

```text
Notification click -> Role C /preview?intent_id=...&restore_scope=same_project
Role C -> Role B POST /resume/select with the deep-link intent_id and scope
Role C displays project, summary, files, URLs, and terminal context
User clicks Open this task and explicitly selects/accepts a mode
Role C -> Role A POST /v1/restore with the unchanged preview resume_payload + mode
```

`POST /resume/select` accepts at least one of `intent_id`, `project_tag`, or `query`, plus optional `restore_scope: "same_project"`. It returns ranked candidates and `needs_picker`; when that flag is true, Role C must require a user choice before displaying any selected payload. The selected preview includes the stored intent ID, label, summary, project/workspace root, and bounded `resume_payload`.

The notification only opens this preview flow through a configured Intent-OS-owned launcher. It never restores applications. The manual path is also required to remain available: search -> select -> review -> explicit confirmation -> restore. Role C must only send the unmodified `resume_payload` returned by Role B or another explicitly approved deterministic source. Every Role A restore requires an explicit user confirmation click; Role A owns validation and restore execution.

## 7. Environment and feature flags

- If Copilot returns 503 `copilot_not_configured`, hide or disable Copilot UI while continuing to show the intent tree, search, and timeline.
- If Copilot returns 502 with `detail: "Copilot provider request failed"`, preserve the Copilot panel and show a retryable “Copilot is temporarily unavailable” state. Continue showing deterministic intent, timeline, and search data.
- If Copilot returns HTTP 200 with `evidence_status: "insufficient"`, show an empty-evidence message rather than an error state; users can refine the question or seed more activity data.
- Prediction may return `null` unless Role B is started with `ENABLE_PREDICTION=true` and sufficient history exists.
- Copilot requires `ENABLE_COPILOT=true`, `ROLE_B_LLM_ENABLED=true`, and a provider key:
  - `LLM_PROVIDER=openai` + `OPENAI_API_KEY`, or
  - `LLM_PROVIDER=gemini` + `GEMINI_API_KEY`
- Optional model override: `INTENT_OS_LLM_MODEL` (defaults: `gpt-4o-mini` / `gemini-2.5-flash`).

## 8. Fixture and demo date

- Locked demo date: **2026-07-13**.
- Fixture: [`role-b/tests/fixtures/demo-day.json`](role-b/tests/fixtures/demo-day.json).
- Expected story: infrastructure work involving IAM and Terraform, represented as a parent intent with child work tasks and deterministic resume context.

## 9. Links

- [`role-b/README.md`](role-b/README.md) — setup and local operations.
- [`role-b/docs/PIPELINE.md`](role-b/docs/PIPELINE.md) — architecture and deterministic/LLM layers.
- [`ROLE-B-COPILOT-EXECUTION-PLAN.md`](ROLE-B-COPILOT-EXECUTION-PLAN.md) — Copilot plan.
- [`ROLE-B-IMPLEMENTATION-PLAN.md`](ROLE-B-IMPLEMENTATION-PLAN.md) — original Role B contract.

## 10. Role C kickoff checklist

- Confirm Role C's UI port and verify it is one of Role B's approved CORS origins.
- Confirm whether Copilot is part of the primary presentation path or an optional panel.
- Confirm the restore UX wording and whether the action is `resume` or `continue`.
- Confirm who seeds demo intents before the presentation and whether replay or live Role A is used.
- Confirm the exact Role A `POST /v1/restore` request contract with the Role A engineer before wiring the Resume CTA.
