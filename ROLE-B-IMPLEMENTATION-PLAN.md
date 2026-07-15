# Intent OS — Role B Implementation Plan

## 1. Mission and boundary

Role B is a portable, local-first intelligence service on port **9478**. It
transforms Role A event JSON into stable, explainable intent trees. It must run
unchanged on Ubuntu, Windows, and in replay/CI environments. It must not launch
apps, inspect windows, run `git`, read Role A's SQLite database in production,
or contain UI code.

The pipeline is:

```text
Role A API or day.json
  → validate + normalize
  → sessionize
  → cluster sub-intents
  → label and summarize
  → derive stats, insights, tags, TODO signals, and resume payloads
  → create parent intents
  → persist/cache
  → localhost API for Role C and Role A's notification/tray
```

Feature scope is governed by `INTENT-OS-FEATURES-SPEC.md`: Tier 1 and Tier 2
are required; Tier 3 is implemented behind stable, deterministic fallbacks.
No cloud account, user/device/session identifier, sync, or OS-specific code is
permitted.

## 2. Producer contract and compatibility

### Inputs

Production live input comes only from Role A HTTP APIs:

```text
GET http://127.0.0.1:9477/v1/events?date=YYYY-MM-DD
GET http://127.0.0.1:9477/v1/events?since=UNIX_SECONDS
GET http://127.0.0.1:9477/v1/export/day?date=YYYY-MM-DD
```

Replay input is a complete `day.json` object passed directly to the same pure
pipeline. Role B never relies on the producer's SQLite layout.

Accept unknown envelope fields and default a missing `schema_version` to `1`.
Use the export's `date` as the authoritative local calendar date. Preserve
input ordering for equal timestamps.

### Source adapter

Current Role A uses `firefox`; older fixtures/specs use `chrome`. Both map to
the internal source family `browser`. `filesystem` events are optional project
signals. Unknown source/type combinations must be retained as `other`, not
rejected.

| Producer event | Internal category | Intent signal |
|---|---|---|
| `linux/app_focus` | `focus` | active app/title and duration evidence |
| `linux/idle_start`, `idle_end` | `idle` | hard/soft session boundary |
| `vscode/workspace_open` | `workspace` | project candidate |
| `vscode/file_open`, `file_edit`, `file_save` | `editor` | file activity and resume file |
| `vscode/document_change` | `editor_detail` | char count, file focus, TODO signal; never send raw text to LLM |
| `firefox` or `chrome` `tab_change`, `tab_close` | `browser` | domain/title research evidence and resume URL |
| `firefox/user_action` | `browser_detail` | reading/research/form-action aggregate only |
| `shell/command` | `command` | cwd, command family, outcome, resume terminal |
| `filesystem/file_modify`, `workspace_seen` | `file_change` | project and asset/file context |

URLs and detailed text may already be redacted by Role A. Role B must treat all
input as potentially redacted and must not attempt to recover it. For a PDF or
image, use a `file_modify` path as strong context; a viewer focus title is only
weak context and must not be presented as an exact file path.

## 3. Repository and modules

```text
role-b/
  intent_engine/
    api.py                 # FastAPI routes, no inference logic
    schemas.py             # input/output Pydantic models
    source.py              # Role A HTTP client and replay reader
    pipeline.py            # pure orchestration entry point
    normalize.py           # event adapter and privacy-safe text
    sessionize.py          # deterministic session boundaries
    cluster.py             # deterministic clustering + optional LLM refinement
    label.py               # provider interface, prompt, fallback labels
    enrich.py              # stats, project tags, insights, TODO observations
    resume.py              # child/parent restore payloads
    prediction.py          # F10 prefix index
    current.py             # F11 60-second cached sliding-window inference
    store.py               # SQLite persistence, cache, FTS search
    logging.py             # local JSONL diagnostics without raw sensitive data
  tests/
    fixtures/              # copied/linked Role A fixture and golden results
    unit/
    integration/
  requirements.txt
  README.md
```

`run_pipeline(export: DayExport) -> list[Intent]` is the core public function.
All HTTP, SQLite, LLM, polling, and replay code wraps this function. It must be
fully testable without network access or a particular operating system.

## 4. Data models

### Normalized event

```python
NormalizedEvent = {
  "id": str,
  "ts": int,
  "ordinal": int,             # original position; tie-breaker only
  "source": str,
  "family": "editor|browser|command|focus|file_change|idle|other",
  "category": str,
  "text": str,                # concise, redaction-safe LLM text
  "entities": {
    "project_paths": list[str],
    "file_path": str | None,
    "file_name": str | None,
    "file_kind": str | None,  # code|pdf|image|other
    "domain": str | None,
    "title": str | None,
    "command": str | None,
    "command_family": str | None,
    "cwd": str | None,
    "exit_code": int | None,
  },
  "signals": {"typed_chars": int, "save": bool, "todo_added": bool},
  "raw": dict                 # never forwarded wholesale to an LLM or logs
}
```

Generate text from facts, for example `Edited iam.tf in project infra`,
`Viewed AWS IAM documentation`, or `Ran terraform apply in infra (failed)`.
For `document_change`, include only the number of inserted characters and a
TODO boolean; never include inserted code or secrets in prompt text.

### Intent response

Every persisted node uses this shape. `children` is populated on roots and is
empty on children; maximum depth is two.

```json
{
  "id": "uuid",
  "parent_id": null,
  "date": "2026-07-13",
  "label": "Deploying Infrastructure",
  "summary": "Updated iam.tf and investigated a failed Terraform apply.",
  "start_ts": 1783911600,
  "end_ts": 1783914300,
  "depth": 0,
  "tags": ["project:infra"],
  "stats": {
    "event_count": 26,
    "duration_seconds": 2700,
    "sources": {"vscode": 8, "firefox": 8, "shell": 5, "linux": 5},
    "unique_apps": ["code", "firefox", "gnome-terminal"]
  },
  "insights": {"editor": [], "browser": [], "shell": []},
  "todos": [],
  "resume_payload": {"files": [], "urls": [], "shell": {}},
  "children": []
}
```

Keep `event_ids`, confidence, labeler version, and source hash in private
storage metadata. Expose them only via a deliberate debug endpoint, never as a
requirement for Role C.

## 5. Inference algorithm

### 5.1 Normalize and enrich local signals

1. Validate minimal envelope fields (`id`, `ts`, `source`, `type`, `payload`).
2. Sort by `(ts, ordinal)` and deduplicate exact repeated event IDs.
3. Create a cross-platform basename by replacing `\\` with `/` before splitting;
   never query the host filesystem.
4. Derive command family from the first safe command token (`terraform`, `git`,
   `pytest`, etc.), URL domain from sanitized URLs, and file extension class.
5. Record all normalized-event validation failures in JSONL and return a typed
   warning in the pipeline result; a malformed event must not discard a day.

### 5.2 Sessionize deterministically

Use a default 15-minute gap. An explicit `idle_start` ends a session; an
`idle_end` begins the next eligible session. Keep a session open across brief
browser/editor switching. Ignore pure focus events as a reason to create a new
session unless they occur after the idle/gap threshold.

Return empty input as `[]`; return a single event as a one-event session. The
threshold is an injected configuration value, never an OS assumption.

### 5.3 Derive project context

Majority vote across `shell.cwd`, VS Code workspace/folder, editor path parent,
and `filesystem.workspace`. Normalize separators and use the final non-empty
path segment as the display tag, e.g. `project:infra`. If no trustworthy path
exists, omit the tag; do not invent one or run `git`.

### 5.4 Cluster sub-intents

Use a deterministic clustering pass first, with an LLM as a constrained
refinement rather than the sole source of truth:

1. Start with chronological runs where adjacent events are ≤5 minutes apart.
2. Maintain each run's weighted topic keys: project, file names, command family,
   browser domains/titles, and viewer asset names.
3. Split only after a sustained topic shift (two consecutive strong signals or a
   command/project change). Attach generic focus events to the nearest cluster.
4. Merge adjacent clusters whose project and topic score match. Cap each session
   at four clusters, merging the smallest adjacent clusters when needed.
5. Enforce invariants: every non-idle event appears exactly once, no overlap,
   chronological start/end, and no empty cluster.

When a configured LLM is available, submit only a numbered list of normalized
text and require a strict JSON partition of event indices. Validate that every
index appears exactly once. If validation, timeout, or rate limit fails, retain
the deterministic clusters. The LLM must not be able to move events across
sessions.

### 5.5 Label, summarize, and create nested intents (F1)

For every child cluster, use one JSON-schema constrained label call:

```json
{"label":"2–5 word goal", "summary":"one factual sentence", "confidence":0.0}
```

Rules: goal-oriented labels, title case for display, label ≤50 characters,
summary based only on supplied facts, and explicit mention of a failed command
when present. Retry malformed output once. Deterministic fallback labels derive
from command family/file/topic: `Run Terraform Apply`, `Edit IAM Trust Policy`,
`Research Documentation`, or `Work Session`.

If a session has more than one child, make one parent-label call using only the
child labels/summaries. Its fallback is a project-aware session label such as
`Infrastructure Work`. The parent aggregates all events, stats, insights, tags,
and resume context. Its children remain exact sub-intents. A one-cluster
session is returned as a root child-equivalent node with `depth: 0`.

This produces the required demo tree:

```text
Deploying Infrastructure
├─ Edit IAM Trust Policy
└─ Run Terraform Apply
```

### 5.6 Resume payloads (F1/F9 contract)

For each child, dedupe by most recent timestamp and cap `files` at 5 and `urls`
at 8. Include VS Code paths, browser URLs, and most recent shell `cwd` plus
`last_cmd`. Exclude missing, redacted, non-HTTP(S), internal-browser URLs, and
unsafe/empty values. The parent payload is the recency-ordered union of its
children plus its latest valid shell state.

Role B only emits payloads; it never chooses `resume` vs `continue` or launches
anything. Role C sends the selected mode to Role A's `POST /restore`.

### 5.7 Statistics and insights (F5/F6/F8/F12)

Compute all enrichment deterministically:

- Stats: event count, duration, source histogram, and normalized unique apps.
- Editor: typed-character total, dominant path, saves per path, and longest
  observed same-file activity span.
- Browser: unique page/domain count, documentation-domain count, repeated
  search terms from safe titles/domains, and reading-mode when no form submit.
- Shell: failed commands with exit status; never include raw stderr in output.
- Project tag: majority-voted `project:<name>`.
- TODOs: detect `TODO|FIXME|XXX` in consented inserted text in memory, persist
  only `{path, observed_ts, marker}`. Since delete events lack deleted text,
  call this an **observed TODO**, not an assuredly open TODO. Support reliable
  closure only if Role A later provides a privacy-safe `todo_removed` signal.

No enrichment feature requires an LLM. Missing detailed capture simply yields
empty insight sections.

### 5.8 Current intent and prediction (F10/F11)

`current.py` polls Role A's incremental events every 60 seconds, runs the same
normalizer/sessionizer on the latest 30-minute window, and caches the result
for 60 seconds. Return `null` when confidence is below 0.5; never display a
confident-sounding guess for sparse data.

For prediction, persist a normalized three-event prefix for each historical
child intent: `(family, category, command_family/project when available)`. A
match must occur for at least two past intents and meet a configurable
confidence threshold. Prediction only preloads a prior resume payload; it does
not restore anything itself. It is disabled by default until rehearsal fixtures
prove it reliable.

## 6. Persistence, idempotence, and caching

Use a separate local SQLite database owned by Role B. Suggested tables:

```sql
pipeline_runs(date TEXT, source_hash TEXT, status TEXT, warnings_json TEXT,
              started_at INTEGER, completed_at INTEGER,
              PRIMARY KEY(date, source_hash));
intents(id TEXT PRIMARY KEY, date TEXT, parent_id TEXT NULL, start_ts INTEGER,
        end_ts INTEGER, label TEXT, summary TEXT, depth INTEGER,
        intent_json TEXT, source_hash TEXT, created_at INTEGER);
intent_search USING fts5(id UNINDEXED, label, summary, insights, tags);
intent_prefixes(intent_id TEXT, prefix_json TEXT, confidence REAL);
```

Hash canonical normalized input events. For the same `(date, hash, pipeline
version)`, return cached results and do not call an LLM again. A recompute for a
changed day replaces that day's intents atomically in one transaction. Keep
precomputed `demo-intents.json` as a release artifact for the demo/replay path.

If FTS5 is unavailable, use a parameterized SQLite `LIKE` fallback. Never build
SQL with the search string.

## 7. HTTP API contract (port 9478)

| Endpoint | Contract |
|---|---|
| `GET /healthz` | `{ok, version, pipeline_version}` |
| `GET /intents/yesterday` | root intent tree for yesterday; used by notification/tray and UI |
| `GET /intents?date=YYYY-MM-DD` | root trees for date |
| `GET /intents/{id}` | one node with insights, TODO observations, and children when root |
| `GET /intents/search?q=&limit=10` | F7 result `{id,label,summary,date,highlight_snippet}`; ≤200 ms cached fixture |
| `GET /intents/current` | F11 `{label,summary,confidence,since_ts}` or `null` |
| `POST /pipeline/run?date=` | fetch Role A export, infer/cache, return roots and warnings |
| `POST /pipeline/run-replay` | accept a `day.json` body, infer/cache; no network dependency |
| `POST /pipeline/recompute?date=` | force refresh; development/admin-local only |
| `GET /intents/prediction` | optional F10 response or `null`; disabled unless feature flag on |

All date parsing is strict ISO `YYYY-MM-DD`. Empty input returns `200 []` from
read routes and a successful run result with zero events. Role A unavailability
returns `503` with a concise local diagnostic. Do not expose API keys, raw
document-change text, or unredacted raw events in these routes.

## 8. Delivery sequence

### Phase 0 — Contract first (half day)

Create the project, typed schemas, Role A HTTP/replay readers, JSONL logging,
and contract fixtures. Add compatibility tests for current `firefox` plus
legacy `chrome` sources, unknown future events, redacted URLs, missing detailed
events, and equal timestamps.

### Phase 1 — Deterministic core (day 1)

Implement pure normalization, sessionization, project tags, clustering,
statistics, and child/parent resume payload union. Use the 26-event fixture as
the golden case. This phase must already create the nested infrastructure tree
without an LLM.

### Phase 2 — Persistence and essential API (day 2)

Implement transactional SQLite storage, source hashes, cached reads, replay
run, live export run, and `GET /intents`, `/yesterday`, `/{id}`. Produce and
commit the precomputed demo-intents artifact only after golden review.

### Phase 3 — Semantic quality (day 3)

Add provider-neutral structured LLM labeler with deterministic fallback and
strict response validation. Add parent labels, F5 stats, F6 insights, and F8
project tags. Golden tests must pass with LLM disabled and mocked.

### Phase 4 — Search and integration (day 4)

Add FTS search, highlight snippets, CORS for the local UI, stable API OpenAPI
examples, and an integration rehearsal with Role A fixture export and Role C.
Meet response targets from cached data before accepting new work.

### Phase 5 — Gated advanced features (day 4–5)

Implement F11 current intent first. Enable F10 prediction only with two seeded
historical repetitions and a rehearsal pass. Implement F12 as observed-TODO
only unless a removal signal is available. Freeze features 48 hours before
submission; thereafter change only bugs, fixtures, and demo artifacts.

## 9. Test and acceptance matrix

| Test | Required result |
|---|---|
| Pure fixture pipeline | 26 events → parent `Deploying Infrastructure` and at least two children |
| Child labels | includes IAM editing and failed Terraform apply, with deterministic fallback |
| Parent resume | union opens `iam.tf`, relevant HTTPS tabs, and `/work/infra` shell context |
| Child resume | contains only that child's context |
| Stats | all node event counts/source totals are correct; parent aggregates children |
| Insights | `iam.tf`, repeated IAM/AccessDenied research, and failed apply are surfaced when detailed events exist |
| Project tag | fixture yields `project:infra` without filesystem/git calls |
| Search | `terraform` returns the fixture node in <200 ms from cache |
| Replay/live parity | same normalized events produce equivalent trees regardless of API or day.json source |
| Unknown/redacted event | pipeline warns but does not fail or leak data |
| PDF/image case | viewer focus is weak context; modified approved asset path becomes a project signal; no content is read |
| Current/prediction | sparse input yields `null`; seeded repeat is required before a prediction is returned |
| API performance | cached `GET /intents` <500 ms on fixture data |

## 10. Explicit non-goals and handoff

Role B does not implement connectors, tray, native notifications, app restore,
filesystem probing, Git root lookup, UI, authentication, cloud synchronization,
or cross-device identity. It emits only an intent response and resume payload
for Role C/Role A to consume.

Before implementation, the Role B owner should confirm only these two handoff
items with Role A:

1. Role A's production event source is `firefox`, while old documentation uses
   `chrome`; the adapter supports both and emits `browser` internally.
2. Detailed events are opt-in and redacted. Their absence is normal and must
   never prevent clustering, labeling, resume extraction, or demo replay.
