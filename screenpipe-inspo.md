# Intent OS — Screenpipe-Inspired Improvements (Codex Spec)

**Purpose:** Feed this document to Codex when implementing patterns borrowed from [screenpipe](https://github.com/screenpipe/screenpipe). Tasks are split by **Role A**, **Role B**, and **Role C** with clear acceptance criteria and copy-paste prompts.

**Reference repo (read-only inspiration):** `screenpipe/`  
**Implementation repos:** `school/OpenAI-Build-Week-2026/role-a/` (Role A) · `school/role-b/` (Role B) · `intent-ui/` (Role C, TBD)

**Canonical Role B spec:** `school/role-b.md` — architecture, modules, API contract, and delivery phases. **If this doc conflicts with role-b.md on Role B, role-b.md wins.**

**Product framing:** Download-and-play Ubuntu app. **Hero verbs:** Capture · Infer · Resume.  
**Hackathon scope:** One local data dir, one demo story, ship features — skip enterprise hygiene (dev/prod split, redaction workers) unless time remains.

**Golden demo scenario:** **Building Login Feature** — relatable for a 90s video (code + browser research + failing test). Not Terraform/infra.

| Beat | What happened | Capture signal |
|------|---------------|----------------|
| Parent intent | **Building Login Feature** | Full afternoon session |
| Child 1 | **Edit Auth Component** | VS Code on `src/auth.tsx` |
| Child 2 | **Fix Failing Tests** | `npm test` exit 1 in `~/projects/taskflow-app` |
| Browser | JWT / Fetch API research | MDN + Stack Overflow tabs |
| Blocked moment | Tests still red | Failed `npm test` in insights |
| Resume | Pick up where you left off | Opens `auth.tsx`, docs tab, terminal cwd |
| Search keyword | `login` or `auth` | Finds parent intent |
| Project tag | `project:taskflow-app` | From shell cwd majority vote |

**Do NOT rebuild screenpipe** (no OCR, no audio, no screen video, no Rust port).

**Related docs:**

- `school/role-b.md` — **Role B source of truth** (Saloni & Mokshita)
- `INTENT-OS-EXECUTION-PLAN.md` — team plan
- `INTENT-OS-ROLE-A-CODEX-SPEC.md` — Role A baseline
- `INTENT-OS-FEATURES-SPEC.md` — feature backlog (F1–F12)

**Ports (locked):**

- Role A event server: `http://127.0.0.1:9477`
- Role B intent engine: `http://127.0.0.1:9478`
- Role C UI (if web): `http://127.0.0.1:9479`

---

## 0. Codex Global Rules

1. **Borrow patterns, not code.** Reimplement in Python/TS to match Intent OS conventions. Do not add Rust dependencies.
2. **One task per Codex session.** Finish acceptance criteria before starting the next task.
3. **No scope creep into screenpipe territory.** If a task mentions OCR/audio/video — skip it.
4. **Every task must map to Capture, Infer, or Resume.**
5. **Test with Role A `demo-day.json` (≥26 events)** and golden `demo-intents.json` from Role B.
6. **Role B logs** to local JSONL via `intent_engine/logging.py` — never log raw document text.

---

## Team Delegation (read first)


| Role  | Owners                | Works on                                                                                      | Dev machine                        |
| ----- | --------------------- | --------------------------------------------------------------------------------------------- | ---------------------------------- |
| **A** | Pranav                | OS capture, event server, restore, `.deb`                                                     | Ubuntu X11                         |
| **B** | **Saloni & Mokshita** | **All OS-agnostic intelligence** — normalize, sessionize, cluster, label, enrich, search, MCP | **Windows OK** — replay `day.json` |
| **C** | **Pranav**            | **UX + frontend only** — display B's API, Resume/Continue buttons, demo polish                | Ubuntu (uses B on localhost)       |


**Boundary rule:** Anything that infers intent, clusters events, calls an LLM, computes stats/insights/tags, or runs without a specific OS API belongs in **Role B**. Role A captures and restores; Role C renders JSON and forwards restore payloads — **no inference in C**.

**Role B never:** launches apps, reads Role A's SQLite in production, runs `git`, probes the filesystem, or ships UI code.

**Role B inputs (production):** Role A HTTP only — `GET /v1/events`, `/v1/export/day`. Replay: pass `day.json` to `POST /pipeline/run-replay`. **firefox** and legacy **chrome** both map to internal `browser`.

**Handoff artifacts:**

- `role-a/fixtures/demo-day.json` (≥26 events, golden case)
- `role-b/tests/fixtures/demo-intents.json` — precomputed after B-P2 golden review

**Communication contract:** Role B owns port **9478**. Role C calls B's HTTP API only — never B's SQLite, never Role A's event DB.

**Task ID prefixes:**

- `B-P`* — delivery phases from `school/role-b.md` (do in order)
- `B-SP*` — screenpipe-inspired add-ons (fit inside phases where noted)
- `C-BOOT*` / `C-F*` / `C-SP*` — Pranav frontend tasks only

**Priority legend:** **P0** = demo blocker · **P1** = demo wow · **P2** = if time · **P3** = stretch

### Suggested split (Saloni & Mokshita — Role B only)


| Person       | Focus         | Day 1–2                                      | Day 3–4                                    | Day 5                   |
| ------------ | ------------- | -------------------------------------------- | ------------------------------------------ | ----------------------- |
| **Saloni**   | Pipeline core | B-P0 + B-P1 normalize/sessionize/cluster     | B-P3 LLM labeler + enrich                  | B-P5 F11 current intent |
| **Mokshita** | API + store   | B-P2 SQLite + HTTP API + demo-intents export | B-P4 FTS search + CORS + integration tests | B-SP4 MCP (optional)    |


Sync daily on intent JSON shape and golden fixture results.

---

## Role B — Master Checklist (Saloni & Mokshita)

**Follow `school/role-b.md` for full detail.** Check off in phase order.


| Priority | ID    | Task                                                                                       | Covers (FEATURES)      | Depends on |
| -------- | ----- | ------------------------------------------------------------------------------------------ | ---------------------- | ---------- |
| P0       | B-P0  | Contract: schemas, source adapter, replay reader, JSONL logging                            | —                      | —          |
| P0       | B-P1  | Deterministic core: normalize → sessionize → cluster → parent/child tree → resume payloads | F1, F5, F8, F9 payload | B-P0       |
| P0       | B-P2  | SQLite store, source-hash cache, API, `demo-intents.json` artifact                         | F1 API                 | B-P1       |
| P1       | B-P3  | LLM labeler + fallback; `enrich.py` stats/insights/TODO observations                       | F5, F6, F8, F12        | B-P1       |
| P1       | B-P4  | FTS search (F7), CORS, OpenAPI, integration rehearsal with A + C                           | F7                     | B-P2       |
| P2       | B-P5  | Gated: F11 current intent, F10 prediction (flag off default), F12 observed TODO            | F10, F11, F12          | B-P3, B-P4 |
| P1       | B-SP4 | Local MCP server (`intent-os-mcp`)                                                         | agent demo             | B-P4       |
| P1       | B-SP7 | Daily digest endpoint (`GET /intents/digest`)                                              | morning briefing       | B-P3       |
| P1       | B-SP8 | Compact agent context (`GET /intents/{id}/context`)                                        | MCP + copy button      | B-P3       |
| P2       | B-SP5 | Pinned intent memories (stretch)                                                           | —                      | B-P2       |
| P3       | B-SP6 | Sessionize on `idle_start`/`idle_end` events (OS-agnostic)                                 | —                      | B-P1       |


**Screenpipe borrows folded into phases (not separate blockers):**

- B-SP1 compact outline → numbered normalized text in `cluster.py` / `label.py` (B-P1/B-P3)
- B-SP2 activity summary → `enrich.py` deterministic stats + insights (B-P1/B-P3)
- B-SP3 FTS search → `store.py` + `GET /intents/search` (B-P4)
- B-SP7 daily digest → `GET /intents/digest` (B-P3)
- B-SP8 agent context → `GET /intents/{id}/context` markdown for MCP/clipboard (B-P3)

**Golden acceptance:** 26-event **login-feature** fixture → parent `Building Login Feature` + ≥2 children (`Edit Auth Component`, `Fix Failing Tests`); parent resume opens `auth.tsx`, MDN/Stack Overflow tabs, `~/projects/taskflow-app` shell; search `login` <200ms cached.

---

## Role C — Master Checklist (Pranav — UX only)

**No inference.** Consume Role B JSON; call Role A restore. Pranav owns A + C on Ubuntu.


| Priority | ID      | Task                                                             | Depends on      |
| -------- | ------- | ---------------------------------------------------------------- | --------------- |
| P0       | C-BOOT1 | App scaffold + load `demo-intents.json` fixture                  | —               |
| P0       | C-BOOT2 | Wire `GET /intents/yesterday`, `/{id}` from :9478                | B-P2            |
| P0       | C-F1    | Nested intent tree (display B's `children[]`)                    | C-BOOT2         |
| P0       | C-F2    | Timeline view                                                    | C-BOOT2         |
| P0       | C-F5    | Stats line on cards (display B's `stats`)                        | C-BOOT2         |
| P0       | C-SP1   | Progressive disclosure layout                                    | C-F5            |
| P1       | C-F6    | Insights panel (display B's `insights`)                          | B-P3            |
| P1       | C-SP3   | Ctrl+K search modal → B `GET /intents/search`                    | B-P4            |
| P1       | C-F8    | Tag chips (display B's `tags[]`)                                 | B-P3            |
| P1       | C-F9    | Resume + Continue → `POST /v1/restore` with B's `resume_payload` | C-BOOT2, Role A |
| P1       | C-SP2   | Onboarding wizard (copy + poll A `/v1/status`)                   | A-SP7           |
| P1       | C-SP4   | Source health panel (display A status)                           | A-SP7           |
| P2       | C-F10   | Prediction banner (display B `GET /intents/prediction`)          | B-P5            |
| P2       | C-F11   | Live intent header (display B `GET /intents/current`)            | B-P5            |
| P1       | C-SP7   | Yesterday digest hero card → B `GET /intents/digest`               | B-SP7           |
| P1       | C-SP8   | Copy agent context button → B `GET /intents/{id}/context`          | B-SP8           |
| P2       | C-SP5   | Pin button UI                                                    | B-SP5           |
| P2       | C-SP6   | MCP setup copy screen                                            | B-SP4           |
| P2       | C-F3    | Morning greeting + empty states                                  | C-BOOT2         |
| P3       | C-F12   | TODO callout (display B's `todos[]`)                             | B-P5            |
| P0       | C-DEMO1 | Demo video polish (loading, error, empty)                        | Day 5           |


---

## 1. What We're Borrowing (Summary)


| Screenpipe concept                                  | Intent OS adaptation                             | Primary role |
| --------------------------------------------------- | ------------------------------------------------ | ------------ |
| Domain URL blocklist (`url_filter.rs`)              | Block sensitive domains at ingest                | A            |
| Activity feed / idle detection (`activity_feed.rs`) | `idle_start`/`idle_end` events → sessionize (B)  | A → B        |
| Activity summary (`activity-summary` MCP tool)      | `enrich.py` per-intent stats + insights          | B            |
| **Daily digest / briefing** (MCP + UI patterns)     | **`GET /intents/digest` — 2–3 sentence yesterday summary** | B + C |
| Compact outline for LLM (91% token cut)             | Numbered normalized-event lines in cluster/label   | B            |
| **find-context / agent context** (MCP patterns)     | **`GET /intents/{id}/context` — resume-safe markdown blob** | B + C |
| FTS5 search (`screenpipe-db`)                       | `store.py` intent search (F7)                    | B            |
| Local MCP server (`screenpipe-mcp`)                 | `intent-os-mcp` — list, search, context, resume  | B            |
| Persistent memories (`update-memory`)               | Pinned intents (B-SP5 stretch)                   | B            |
| Progressive disclosure (`AGENTS.md`, `DESIGN.md`)   | UI: summary first, details on expand             | C (Pranav)   |
| Onboarding + source status                          | Wizard displays A status; no capture logic       | A + C        |
| **Example agent prompts** (Connections UI)          | MCP setup + "Resume my login work" copy-paste    | C            |
| **Copy-to-clipboard context** (agent workflows)   | C-SP8 one-click context for Claude/Codex         | C            |
| AT-SPI2 Linux a11y (`screenpipe-a11y`)              | Richer desktop events (stretch)                  | A            |
| Dev vs prod data dirs (`ONBOARDING.md`)             | **Deferred post-hackathon** — single DB path OK  | —            |
| Background redaction worker (`screenpipe-redact`)   | **Deferred** — sync redact at ingest is enough   | A            |


**Explicitly NOT borrowing:** screen OCR, audio transcription, pipes scheduler (defer), cloud sync, team tier, ONNX PII models.

---

## 2. Task Dependency Graph

```
B-P0 → B-P1 → B-P2 ──┬──► B-P3 → B-P5 (F10/F11/F12 gated)
                     └──► B-P4 (search) ──► B-SP4 MCP

A demo-day.json ──► B-P0..P1 (Windows replay, no Ubuntu)
B-P2 demo-intents.json ──► C-BOOT1 (Pranav UI mocks)
B-P2 API ──► C-BOOT2

C-BOOT1 → C-BOOT2 → C-F* / C-SP*   (display only, no inference)

A-SP7 ──► C-SP2, C-SP4              idle_start/end in events ──► B-SP6
```

**Role B start order:** B-P0 → B-P1 → B-P2 → B-P3 → B-P4 → B-P5  
**Role C start order (Pranav):** C-BOOT1 (fixture) → C-BOOT2 (when B-P2) → C-F5 + C-F2 → C-F1 → C-SP1 → C-F9

---

# ROLE A TASKS

**Codebase:** `school/OpenAI-Build-Week-2026/role-a/`  
**Owner:** Pranav  
**Stack:** Python 3.11+, FastAPI, SQLite, systemd --user

---

## A-SP1: Dev vs Production Data Directories — **SKIP (hackathon)**

**Inspired by:** screenpipe `ONBOARDING.md` Part 1

**Status:** **Deferred post-hackathon.** One DB at `~/.local/share/intent-os/events.db` is fine for demo week. Do not spend time on `INTENT_OS_DATABASE`, `dev-reset`, or dual-instance docs unless you wipe real capture data and need a quick fix.

**If already implemented:** leave it; do not expand or test further for submission.

---

## A-SP2: Domain-Level URL Blocklist

**Inspired by:** `screenpipe/crates/screenpipe-a11y/src/url_filter.rs`

**Goal:** Block sensitive URLs at ingest using domain patterns (not substring false positives).

### Tasks

1. Add config file `~/.config/intent-os/blocked-domains.yaml`:

```yaml
blocked_domains:
  - chase.com
  - wellsfargo.com
  - internal.company.com
```

1. Implement `event_server/url_filter.py`:
  - `is_url_blocked(url: str, patterns: list[str]) -> bool`
  - Match rules: exact host, subdomain (`*.pattern`), no-TLD pattern (`chase` → `chase.com`)
  - Port logic from screenpipe's `host_matches_pattern` behavior (exact + subdomain + no-TLD)
2. On `POST /event` for `firefox/tab_change` and `firefox/user_action`:
  - If blocked: either drop event (204) OR store with `payload.url = "[blocked]"` and `payload.blocked = true` (prefer store-with-flag for audit)
3. Expose blocked list in `GET /v1/detailed-capture/config` or new `GET /v1/config` (read-only).

### Acceptance

- `chase.com` blocked; `purchase.com` NOT blocked when pattern is `chase`
- Unit tests in `event_server/tests/test_url_filter.py`
- Example config in `config/blocked-domains.yaml.example`

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md task A-SP2 and screenpipe url_filter.rs for matching rules.
Implement event_server/url_filter.py and wire into ingest for firefox events.
Add tests. Do not block vscode or shell events.
```

---

## A-SP3: Activity Feed (Idle / Burst Detection)

**Inspired by:** `screenpipe/crates/screenpipe-a11y/src/activity_feed.rs`

**Goal:** Lightweight activity signals without storing keystroke content. Feed Role B for better session boundaries and reduce X11 poll rate when idle.

### Tasks

1. Create `collectors/activity/feed.py`:

```python
class ActivityFeed:
    def record(self, kind: str) -> None: ...       # "key", "mouse", "shell", "focus"
    def idle_ms(self) -> int: ...
    def is_typing(self) -> bool: ...              # keyboard activity < 300ms
    def is_keyboard_burst(self) -> bool: ...       # 3+ keys in 500ms window
    def is_active(self, threshold_ms: int) -> bool: ...
    def recommended_poll_interval_sec(self) -> float: ...  # 2.0 active → 10.0 deep idle
```

1. Wire updates from:
  - X11 tracker on focus change → `record("focus")`
  - Shell hook on command → `record("shell")`
  - Optional: read `xprintidle` in tracker loop → update idle state
2. Emit optional events (stretch):

```json
{
  "source": "linux",
  "type": "activity_state",
  "payload": { "idle_ms": 4200, "is_typing": false, "poll_interval_sec": 5.0 }
}
```

   Add to `EVENT_PAYLOAD_FIELDS` in `models.py` if emitting.

1. Expose feed snapshot on `GET /v1/status`:

```json
{
  "activity": { "idle_ms": 4200, "is_typing": false, "recommended_poll_interval_sec": 5.0 }
}
```

1. Update X11 tracker to use dynamic sleep from `recommended_poll_interval_sec()`.

### Acceptance

- Tracker slows polling after 5s idle (log or metric visible)
- `/v1/status` includes activity block
- Tests for `is_keyboard_burst` logic with mocked timestamps

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md task A-SP3 and screenpipe activity_feed.rs.
Implement collectors/activity/feed.py, integrate with x11 tracker and /v1/status.
Add optional linux/activity_state events if models.py updated.
```

---

## A-SP4: JSONL Structured Logging

**Inspired by:** screenpipe operational rigor + Intent OS role A spec §7

**Goal:** Debug demo-day failures with `tail -f | jq`.

### Tasks

1. Create `event_server/logging_setup.py` with `JsonlFormatter` writing to:
  - `~/.local/share/intent-os/logs/event-server.jsonl`
  - `~/.local/share/intent-os/logs/x11-tracker.jsonl` (tracker uses same helper)
2. Log lines:

```json
{"ts":"2026-07-16T10:00:00Z","level":"info","component":"event-server","event":"event_ingested","source":"vscode","type":"file_open","id":"..."}
```

1. Replace bare `print()` in event server and tracker with logger calls.
2. Log errors with `level":"error"` and exception type — never silent except.

### Acceptance

- POST /event produces ingested log line
- Tracker failure produces error log line
- Logs directory created on startup

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md task A-SP4.
Add JSONL logging to event_server and collectors/x11/tracker.py.
Replace print() debug paths with structured logger.
```

---

## A-SP5: Expand Demo Fixture — **Building Login Feature** (P0)

**Inspired by:** screenpipe's rich demo data for search/summary testing

**Goal:** Golden `demo-day.json` for the **login feature** story — better on camera than Terraform.

### Tasks

1. Expand `fixtures/demo-day.json` to **26–30 events** for this arc:
   - **VS Code:** open/edit/save `src/auth.tsx`; optional `document_change` with TODO in auth code
   - **Firefox:** MDN Fetch API doc, Stack Overflow JWT thread, maybe React docs
   - **Shell:** `cd ~/projects/taskflow-app`, `npm test` (exit 1), `npm run dev`
   - **Linux:** focus switches between Code, Firefox, Terminal
   - Optional **idle_start/end** between lunch break (session boundary for B)
2. Paths use `~/projects/taskflow-app` — not `/work/infra` or `.tf` files.
3. `scripts/emit_fixture.py` replays to `POST /v1/event`.

### Acceptance

- `jq '.events | length' fixtures/demo-day.json` ≥ 26
- Events tell a coherent "building login, tests failing, researching JWT" story
- Role B can produce parent **Building Login Feature** from replay alone

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md A-SP5 golden demo scenario (Building Login Feature).
Expand fixtures/demo-day.json with vscode/firefox/shell/linux events for auth.tsx + npm test failure + MDN/Stack Overflow research.
No terraform or .tf files. Verify emit_fixture.py still works.
```

---

## A-SP6: AT-SPI2 Desktop Context (Stretch)

**Inspired by:** `screenpipe/crates/screenpipe-a11y` Linux AT-SPI2 path

**Goal:** Richer desktop events than xdotool title-only. Future-proofs beyond X11.

**Priority:** P3 — only if A-SP4–A-SP7 done and demo stable.

### Tasks

1. Add optional collector `collectors/atspi/context.py` using `pyatspi` or `dashel` (evaluate deps for Ubuntu 22.04).
2. On focus change, emit:

```json
{
  "source": "linux",
  "type": "app_focus",
  "payload": {
    "app": "firefox",
    "title": "...",
    "focused_role": "document",
    "focused_name": "Sign in",
    "is_password_field": false
  }
}
```

1. **Never capture password fields** — skip when `is_password_field` or AT-SPI state indicates sensitivity.
2. Gate behind config `desktop_capture: atspi | x11` default `x11`.

### Acceptance

- AT-SPI mode emits richer payload on Firefox focus
- Password field focus emits nothing
- Falls back to x11 if pyatspi unavailable

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md task A-SP6.
Implement optional AT-SPI focus collector gated by config, with password field skip.
Keep x11 tracker as default fallback.
```

---

## A-SP7: Enhanced Source Status for Onboarding

**Inspired by:** screenpipe health-check + permissions status

**Goal:** Role C onboarding shows which connectors are alive.

### Tasks

1. Extend `GET /v1/status` response:

```json
{
  "ok": true,
  "session_type": "x11",
  "sources": {
    "vscode": { "event_count": 12, "last_event_ts": 1720870500, "healthy": true },
    "firefox": { "event_count": 8, "last_event_ts": 1720870600, "healthy": true },
    "shell": { "event_count": 3, "last_event_ts": 1720870700, "healthy": false },
    "linux": { "event_count": 5, "last_event_ts": 1720870400, "healthy": true }
  },
  "services": {
    "event_server": true,
    "x11_tracker": true,
    "workspace_watch": false
  },
  "activity": { "idle_ms": 0, "is_typing": false }
}
```

1. `healthy: false` if no events from source in last 30 minutes AND capture enabled (configurable staleness).
2. Document in `docs/TROUBLESHOOTING.md` per-source fixes.

### Acceptance

- Status reflects fixture replay counts
- Stale source marked unhealthy after threshold

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md task A-SP7.
Extend GET /v1/status with healthy flags and service booleans.
Update docs/TROUBLESHOOTING.md with one paragraph per source.
```

---

## A-SP8: Background Redaction Queue — **SKIP (hackathon)**

**Inspired by:** `screenpipe-redact`

**Status:** **Deferred.** Sync redact at ingest (Role A) is enough for demo week. Do not build a background worker unless ingest latency is measurably broken.

---

# ROLE B TASKS — Intent Engine (Saloni & Mokshita)

**Owners:** Saloni & Mokshita  
**Canonical spec:** `school/role-b.md` — read first; implement that document. This section adds Codex prompts and screenpipe borrow notes.

**Mission:** Portable, local-first **intelligence service** on port **9478**. Transform Role A event JSON into stable intent trees. Must run unchanged on Ubuntu, Windows, and CI replay. **All OS-agnostic inference lives here.**

**Stack:** Python 3.11+, FastAPI, Pydantic, SQLite (Role B's own DB), optional OpenAI for labels  
**Repo:** `school/role-b/` with package `intent_engine/`

**Hard boundaries (non-goals):** no UI, no app launch/restore, no Role A SQLite reads in production, no `git`, no host filesystem probes, no cloud sync, no `device_id`.

**Producer contract:**

```text
GET http://127.0.0.1:9477/v1/events?date=YYYY-MM-DD
GET http://127.0.0.1:9477/v1/events?since=UNIX_SECONDS
GET http://127.0.0.1:9477/v1/export/day?date=YYYY-MM-DD
POST /pipeline/run-replay  ← accepts day.json body (Windows dev path)
```

Map `firefox` and legacy `chrome` → internal `browser`. Unknown events → `other`, never reject the day.

**Core public function:** `run_pipeline(export: DayExport) -> list[Intent]` — pure, testable without network or OS.

**Module layout (from role-b.md):**

```text
role-b/intent_engine/
  api.py           schemas.py       source.py        pipeline.py
  normalize.py     sessionize.py    cluster.py       label.py
  enrich.py        resume.py        prediction.py    current.py
  store.py         logging.py
tests/fixtures/    unit/              integration/
```

**Intent API (port 9478):** see role-b.md §7 — `GET /healthz`, `/intents/yesterday`, `/intents?date=`, `/intents/{id}`, `/intents/search`, `/intents/current`, `/intents/prediction`, `POST /pipeline/run`, `/pipeline/run-replay`, `/pipeline/recompute`.

---

## B-P0: Contract First (P0, ~half day)

**Ref:** role-b.md §8 Phase 0

### Tasks

1. Create `school/role-b/` with typed Pydantic schemas (`schemas.py`) matching intent response shape (§4).
2. `source.py`: Role A HTTP client + replay reader for `day.json`; never read Role A SQLite.
3. `logging.py`: local JSONL diagnostics — no raw `document_change` text, no API keys.
4. `GET /healthz` → `{ ok, version, pipeline_version }`.
5. Contract tests: `firefox` + legacy `chrome`, unknown future events, redacted URLs, missing detailed events, equal timestamps.

### Acceptance

- `pytest tests/unit/` passes on Windows without Role A running
- Replay loads 26-event golden fixture
- Malformed single event → warning, day continues

### Codex prompt

```
Read school/role-b.md sections 2, 3, 4, 8 Phase 0, and INTENT-OS-SCREENPIPE-BORROWS.md B-P0.
Scaffold school/role-b/intent_engine/ with schemas, source.py replay reader, logging.py, GET /healthz.
Add contract tests for firefox/chrome adapter and unknown events.
```

---

## B-P1: Deterministic Core (P0, day 1)

**Ref:** role-b.md §5.1–5.7, §8 Phase 1 · **Features: F1, F5, F8, F9 payload**

**Goal:** Full nested intent tree **without LLM** — golden demo must pass with LLM disabled.

### Tasks

1. `**normalize.py`:** Map producer events → `NormalizedEvent` (family, category, redaction-safe `text`, entities). Cross-platform basename via `/` only — never touch host FS. `document_change`: char count + TODO flag only, never raw insert text to LLM.
2. `**sessionize.py`:** 15-min gap default; `idle_start` ends session, `idle_end` starts next. Injected threshold, not OS-specific. (B-SP6 extends this.)
3. `**cluster.py`:** Deterministic clustering first — chronological runs ≤5 min, topic keys, split on sustained shift, merge adjacent, cap 4 clusters/session. LLM is refinement only (B-P3).
4. `**label.py` (fallback mode):** Deterministic labels — `Fix Failing Tests`, `Edit Auth Component`, `Research JWT Docs`, etc.
5. **Nested intents (F1):** Multi-cluster session → parent aggregates stats/insights/tags/resume; children are exact sub-intents.
6. `**resume.py`:** Child payloads dedupe by recency; parent = union of children. Role B emits payload only.
7. **`enrich.py` (deterministic):** stats, project tag majority vote (`project:taskflow-app` — **no git**), insights, observed TODO.
8. `**pipeline.py`:** `run_pipeline(export) -> list[Intent]` orchestrator.
9. **B-SP1:** LLM prompts use numbered normalized `text` lines, not raw JSON.

### Acceptance

- 26 events → parent `Building Login Feature` + ≥2 children
- Parent resume: `auth.tsx`, MDN/Stack Overflow URLs, shell cwd `~/projects/taskflow-app`
- `project:taskflow-app` without git/filesystem calls
- Pipeline passes with `LLM_ENABLED=false`

### Codex prompt

```
Read school/role-b.md sections 5.1–5.7 and 8 Phase 1, and INTENT-OS-SCREENPIPE-BORROWS.md B-P1.
Implement normalize, sessionize, deterministic cluster, fallback label, enrich, resume, pipeline.py.
Golden test: 26-event fixture → nested login-feature tree without LLM.
```

---

## B-P2: Persistence + Essential API (P0, day 2)

**Ref:** role-b.md §6, §7, §8 Phase 2

### Tasks

1. `**store.py`:** SQLite `pipeline_runs`, `intents`, `intent_search` (FTS5 stub OK). Source-hash cache.
2. Atomic day replace on recompute.
3. `api.py` routes: `GET /intents/yesterday`, `/intents?date=`, `/intents/{id}`, `POST /pipeline/run`, `/pipeline/run-replay`, `/pipeline/recompute` (dev only).
4. Export `tests/fixtures/demo-intents.json` after golden review — unblocks Pranav C-BOOT1.

### Acceptance

- Cached `GET /intents/yesterday` <500 ms
- Replay and live export produce equivalent trees
- Role A down → `503`

### Codex prompt

```
Read school/role-b.md sections 6, 7, 8 Phase 2, and INTENT-OS-SCREENPIPE-BORROWS.md B-P2.
Add store.py, intent API routes, demo-intents.json export.
```

---

## B-P3: Semantic Quality (P1, day 3)

**Ref:** role-b.md §5.4–5.5, §5.7, §8 Phase 3 · **F5, F6, F8, F12**

### Tasks

1. LLM labeler with validation + deterministic fallback (`label.py`).
2. Optional LLM cluster refinement on numbered normalized lines only.
3. Complete `enrich.py` insights + **observed** TODO (not assured open).
4. Golden tests with LLM disabled and mocked.

**B-SP2 folded here:** `enrich.py` replaces separate `activity_summary.py`.

### Acceptance

- Insights surface `auth.tsx`, JWT/Fetch API research, failed `npm test` when detailed events exist
- LLM failure → deterministic tree unchanged

### Codex prompt

```
Read school/role-b.md sections 5.4, 5.5, 5.7, 8 Phase 3, and INTENT-OS-SCREENPIPE-BORROWS.md B-P3.
Add LLM labeler with fallback and complete enrich.py.
```

---

## B-P4: Search + Integration (P1, day 4)

**Ref:** role-b.md §8 Phase 4 · **F7**

### Tasks

1. FTS5 + query sanitizer in `store.py`; `LIKE` fallback if FTS5 unavailable.
2. `GET /intents/search?q=` ≤200 ms cached.
3. CORS for local UI; OpenAPI examples.
4. Integration rehearsal with Role A export + Pranav UI.

**B-SP3 folded here.**

### Codex prompt

```
Read school/role-b.md section 8 Phase 4 and INTENT-OS-SCREENPIPE-BORROWS.md B-P4.
Add FTS5 search and CORS.
```

---

## B-P5: Gated Advanced Features (P2, day 4–5)

**Ref:** role-b.md §5.8, §8 Phase 5 · **F10, F11, F12**

### Tasks

1. `current.py` (F11): 30-min window, 60s cache, `null` if confidence <0.5.
2. `prediction.py` (F10): prefix index; **disabled by default** until rehearsal passes.
3. Freeze 48h before submission.

### Codex prompt

```
Read school/role-b.md sections 5.8 and 8 Phase 5, and INTENT-OS-SCREENPIPE-BORROWS.md B-P5.
Implement current.py and prediction.py behind feature flags.
```

---

## B-CORE2 through B-F12 — Superseded

**Do not implement as separate tasks.** Features F1–F12 are delivered inside **B-P1 through B-P5** per `school/role-b.md`:


| Old ID               | Now in                            |
| -------------------- | --------------------------------- |
| B-CORE2–4            | B-P0, B-P1, B-P2                  |
| B-F1 nested intents  | B-P1                              |
| B-F5 stats           | B-P1 `enrich.py`                  |
| B-F6 insights        | B-P1/B-P3 `enrich.py`             |
| B-F8 project tags    | B-P1 (no git)                     |
| B-F10 prediction     | B-P5 `prediction.py`              |
| B-F11 current intent | B-P5 `current.py`                 |
| B-F12 TODO           | B-P3 observed TODO in `enrich.py` |
| B-CORE5 LLM logging  | Optional via `logging.py`         |


---

# ROLE B — Screenpipe-Inspired Add-Ons (B-SP*)

**B-SP1–3 are folded into B-P1/P3/P4** (see phase sections). Only B-SP4–6 are separate optional tasks.

## B-SP1: Compact Event Outline → **folded into B-P1/B-P3**

Numbered normalized-event lines for LLM prompts (~91% token reduction vs raw JSON). Implement in `cluster.py` / `label.py`.

---

## B-SP2: Activity Summary → **folded into B-P1/B-P3 (`enrich.py`)**

Deterministic stats + insights in `enrich.py`. No separate `activity_summary.py` required.

---

## B-SP3: FTS5 Intent Search → **folded into B-P4 (`store.py`)**

See B-P4 acceptance criteria.

---

## B-SP4: Local MCP Server (`intent-os-mcp`)

**Inspired by:** `packages/screenpipe-mcp` — Claude queries local context without custom UI

**Goal:** Highest-ROI integration. Demo line: *"Ask Claude what you were doing yesterday."*

### Tasks

1. Create package `intent-os-mcp/` (Python with `mcp` SDK or Node/Bun mirroring screenpipe structure).
2. Stdio transport (default). HTTP transport stretch.
3. Tools:


| Tool               | Description                               | Backend                                 |
| ------------------ | ----------------------------------------- | --------------------------------------- |
| `list-intents`     | List intents for date (default yesterday) | `GET /intents?date=`                    |
| `get-intent`       | Full intent + insights + resume_payload   | `GET /intents/:id`                      |
| `search-intents`     | Keyword search                            | `GET /intents/search?q=`                |
| `get-intent-context` | Compact markdown for agents               | `GET /intents/{id}/context`             |
| `daily-digest`       | Yesterday summary                         | `GET /intents/digest`                   |
| `resume-intent`      | Trigger restore                           | `POST http://127.0.0.1:9477/v1/restore` |
| `current-intent`     | Live intent if F11 built                  | `GET /intents/current`                  |
| `health-check`       | A + B up?                                 | `GET :9477/v1/status` + `:9478/healthz` |


1. README with Claude Desktop config:

```json
{
  "mcpServers": {
    "intent-os": {
      "command": "python",
      "args": ["-m", "intent_os_mcp"],
      "env": { "INTENT_ENGINE_URL": "http://127.0.0.1:9478" }
    }
  }
}
```

1. No telemetry to cloud in v0.

### Acceptance

- MCP inspector lists all tools
- `list-intents` returns demo data from precomputed or live API
- `resume-intent` calls Role A restore (mock in test)
- README install steps work on Ubuntu

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md task B-SP4 and packages/screenpipe-mcp/README.md for structure.
Create intent-os-mcp Python package with stdio MCP server exposing list-intents, get-intent, search-intents, resume-intent, health-check.
Document Claude Desktop config. Add smoke test.
```

---

## B-SP5: Pinned Intent Memories (P2 stretch)

**Inspired by:** screenpipe `update-memory` · **Not in role-b.md v1** — only if B-P5 complete early.

Pin API + `pinned_intents` table; pinned rows survive pipeline recompute.

---

## B-SP6: Idle-Aware Sessionize (P3)

**Inspired by:** screenpipe session boundaries · **OS-agnostic**

**Depends on:** Role A emitting `linux/idle_start` / `idle_end` in event stream (not A-SP3 activity polling)

### Tasks

1. In `sessionize.py`: `idle_start` → force session end; `idle_end` → eligible new session.
2. Keep 15-min gap as default; idle events are hard boundaries.
3. Unit tests with synthetic idle markers in fixture JSON.

### Acceptance

- Session count changes predictably when idle events injected
- No OS-specific activity APIs

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md B-SP6 and school/role-b.md section 5.2.
Update sessionize.py to honor idle_start/idle_end from normalized stream.
```

---

## B-SP7: Daily Digest (P1)

**Inspired by:** screenpipe morning briefing + day-scope activity summary

**Goal:** One-glance "what was yesterday" for UI hero + MCP.

### Tasks

1. `GET /intents/digest?date=YYYY-MM-DD` (default yesterday):

```json
{
  "date": "2026-07-13",
  "headline": "Building Login Feature",
  "summary": "You edited auth.tsx, researched JWT on MDN, and hit failing npm tests.",
  "top_intent_ids": ["uuid-parent"],
  "intent_count": 1,
  "total_duration_seconds": 5400
}
```

2. Build **deterministically** from persisted intents — no extra LLM for v0.
3. Wire MCP tool `daily-digest` in B-SP4.

### Acceptance

- Digest matches login-feature golden fixture in ≤3 factual sentences
- Response <100ms from cache

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md B-SP7.
Add GET /intents/digest built from stored intents. Rule-based summary only.
```

---

## B-SP8: Compact Agent Context (P1)

**Inspired by:** screenpipe MCP context/outline tools

**Goal:** Paste-ready markdown for Claude/Codex — no raw events.

### Tasks

1. `GET /intents/{id}/context` returns resume-safe markdown (~2KB cap):

```markdown
# Building Login Feature
Updated auth.tsx and debugged failing npm tests.

## Resume
- File: ~/projects/taskflow-app/src/auth.tsx
- URL: https://developer.mozilla.org/.../Fetch_API
- Shell: ~/projects/taskflow-app (last: npm test)

## Insights
- Editor: heavy edits on auth.tsx
- Browser: JWT documentation research
- Shell: npm test failed (exit 1)
```

2. Never include raw `document_change` text or secrets.
3. MCP tool `get-intent-context` in B-SP4.

### Acceptance

- Context includes auth.tsx and npm test failure for golden fixture
- Safe to paste into Claude Desktop

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md B-SP8.
Add GET /intents/{id}/context markdown export. No raw events.
```

---

# ROLE C TASKS — UI & Demo (Pranav)

**Owner:** Pranav  
**Scope:** **UX + frontend only.** Display Role B's intent JSON; forward `resume_payload` to Role A restore. **No inference, clustering, LLM, or OS capture logic in C.**

**Stack:** Electron + React **or** Tauri + React (pick one, document in README)  
**Repo:** `school/OpenAI-Build-Week-2026/intent-ui/` (sibling to `role-a/`, `role-b/`)

**APIs (read-only from C's perspective):**

- Intents: `http://127.0.0.1:9478` (Role B — Saloni & Mokshita)
- Restore: `POST http://127.0.0.1:9477/v1/restore` (Role A — Pranav)
- Status: `GET http://127.0.0.1:9477/v1/status` (Role A — for onboarding/health UI only)

**Fixture fallback:** Load `role-b/tests/fixtures/demo-intents.json` when `:9478` unreachable during UI dev.

**Design:** Progressive disclosure (screenpipe-inspired). Summary first, details on expand. See `C-SP1`.

**Do not in Role C:** merge resume payloads client-side (B already unions parent payloads), recompute stats, call OpenAI, read event SQLite.

---

## C-BOOT1: App Scaffold + Mock Data (P0)

**Goal:** Runnable UI showing hardcoded or fixture intents — no Role B required Day 1.

### Tasks

1. Create `intent-ui/` with Electron/Tauri + React + TypeScript
2. Copy `fixtures/demo-intents.json` into `public/` or `src/fixtures/`
3. Pages/routes:
  - `/` Yesterday (home)
  - `/intent/:id` Detail
  - `/settings` Settings shell
4. `IntentCard` component: label, summary, placeholder stats
5. `npm run dev` / `bun dev` starts on Windows

### Acceptance

- App launches on Windows
- Shows ≥1 mock intent from demo-intents.json
- No crash when 9478 offline (fixture mode)

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md C-BOOT1.
Create intent-ui/ Electron or Tauri + React app with Yesterday view loading demo-intents.json.
Works offline on Windows.
```

---

## C-BOOT2: Wire to Role B API (P0)

**Goal:** Live data from port 9478 with fixture fallback.

### Tasks

1. `api/intents.ts`:
  - `fetchYesterdayIntents()` → GET /intents/yesterday
  - `fetchIntent(id)` → GET /intents/:id
  - Fallback to local JSON if fetch fails
2. Env: `INTENT_API_URL=http://127.0.0.1:9478`
3. Loading + error states on Yesterday view

### Acceptance

- With B running: shows live intents
- With B down: shows fixture + banner "offline mode"

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md C-BOOT2.
Wire intent-ui to Role B API with fixture fallback and loading states.
```

---

## C-F1: Nested Intent Tree UI (P0)

**Ref:** FEATURES-SPEC F1 · depends on B-P1 nested API shape

### Tasks

1. Collapsible tree: render B's `children[]` on parent nodes
2. Parent shows B's aggregate `stats` (already computed server-side)
3. **Resume on parent or child** → POST Role A restore with that node's `**resume_payload` from B** (no client-side merge)

### Acceptance

- Tree renders 1 parent + 2 children from demo-intents
- Expand/collapse works

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md C-F1 and FEATURES-SPEC F1.
Add nested intent tree to Yesterday view with per-node Resume.
```

---

## C-F2: Timeline View (P0)

**Ref:** FEATURES-SPEC F2

### Tasks

1. Horizontal timeline component: x-axis = local time for selected date
2. One bar per intent (`start_ts` → `end_ts`)
3. Colors: vscode=blue, firefox=orange, shell=green, mixed=purple (use stats.sources)
4. Click bar → scroll to intent card
5. Date picker (default: yesterday)

### Acceptance

- Timeline shows ≥1 bar for demo data
- Click scrolls to card
- Readable at 1280px width

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md C-F2 and FEATURES-SPEC F2.
Build horizontal timeline on Yesterday view with click-to-scroll.
```

---

## C-F3: Morning Greeting + Empty States (P2)

**Ref:** FEATURES-SPEC F3 (UI portion; notification script is Role A)

### Tasks

1. Header copy: "Good morning. Here's what you were doing yesterday."
2. Empty state: no intents → "No activity recorded. Enable sources in Settings."
3. First-run redirect to onboarding (C-SP2)

### Acceptance

- Empty fixture shows helpful empty state
- Greeting visible on Yesterday when intents exist

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md C-F3.
Add morning greeting header and empty states to Yesterday view.
```

---

## C-F5: Stats Line on Intent Cards (P0)

**Ref:** FEATURES-SPEC F5 · display B's `stats` from B-P1+

### Tasks

1. `formatDuration(seconds)` → "2h 34m"
2. Subtitle: `{duration} · {event_count} events · {unique_apps.length} apps`
3. Optional mini source breakdown bar (stretch)

### Acceptance

- Every card shows stats when API provides them

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md C-F5.
Display intent.stats on IntentCard with human-readable duration.
```

---

## C-F6: Insights Panel (P1)

**Ref:** FEATURES-SPEC F6 · display B's `insights` from B-P3+

### Tasks

1. Intent detail page section "Insights"
2. Bullets grouped: Editor / Browser / Shell
3. Collapsed on card; expanded on detail view

### Acceptance

- Detail shows ≥2 insight bullets for demo intent

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md C-F6.
Add Insights section to intent detail from API insights object.
```

---

## C-F8: Project Tag Chips (P1)

**Ref:** FEATURES-SPEC F8 · display B's `tags[]` (B-P1+)

### Tasks

1. Render `tags[]` as chips on intent card (e.g. `project:taskflow-app`)
2. Optional filter: click chip filters Yesterday list

### Acceptance

- Tag chip visible when API returns tags

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md C-F8.
Show tags as filter chips on intent cards.
```

---

## C-F9: Resume + Continue Buttons (P1)

**Ref:** FEATURES-SPEC F9 · depends on A restore `mode` param

### Tasks

1. **Resume** → `POST /v1/restore` with `{ mode: "resume", ...payload }`
2. **Continue** → `{ mode: "continue", ...payload }`
3. Loading state + toast on success/failure (`failed[]` from API)
4. If A doesn't support mode yet, both call same endpoint (graceful)

### Acceptance

- Resume button triggers restore on Ubuntu integration test
- Both buttons visible on card and detail

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md C-F9 and FEATURES-SPEC F9.
Add Resume and Continue buttons calling Role A restore API.
```

---

## C-F10: Prediction Banner (P2)

**Ref:** FEATURES-SPEC F10 · display B `GET /intents/prediction` (B-P5)

### Tasks

1. On app load poll `GET /intents/predict`
2. Dismissible banner: "Looks like you're starting **{label}** again. Preload?"
3. CTA runs restore with predicted resume_payload

### Acceptance

- Banner shows when predict API returns match in rehearsal

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md C-F10.
Add dismissible prediction banner wired to B-P5 predict endpoint.
```

---

## C-F11: Live Intent Header (P2)

**Ref:** FEATURES-SPEC F11 · display B `GET /intents/current` (B-P5)

### Tasks

1. Poll `GET /intents/current` every 60s
2. Header: "Now: {label}" or "Now: Working…" if null
3. Sync label to window title (optional)

### Acceptance

- Header updates when current endpoint returns label

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md C-F11.
Add live current-intent line in app header with 60s polling.
```

---

## C-F12: TODO Callout (P3)

**Ref:** FEATURES-SPEC F12 · display B's observed `todos[]` (B-P3+)

### Tasks

1. On intent detail, if todos present: callout box with file + TODO text
2. Link opens file path (xdg-open on Linux — show path on Windows dev)

### Acceptance

- TODO callout renders from API todos field

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md C-F12.
Add TODO callout on intent detail when todos returned from API.
```

---

## C-DEMO1: Demo-Ready UI States (P0, Day 5)

**Goal:** Video recording won't show broken UI.

### Tasks

1. Loading skeletons for Yesterday + detail
2. Error boundary + retry button
3. Hide debug URLs / raw JSON
4. Consistent window size for recording (1280×720 or 1920×1080)
5. Dark mode optional; pick one theme for video

### Acceptance

- No blank screens during slow API
- Recording rehearsal looks clean

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md C-DEMO1.
Add loading skeletons, error states, and recording-friendly layout polish.
```

---

# ROLE C — Screenpipe-Inspired Tasks (C-SP*)

## C-SP1: Progressive Disclosure UI

**Inspired by:** screenpipe `AGENTS.md` progressive disclosure + `DESIGN.md` minimalism

**Goal:** Default view is simple; power details on expand. Maps to Capture · Infer · Resume.

### Tasks

1. **Default Yesterday view shows only:**
  - Top intent label + one-line summary
  - Stats line (F5): duration · event count
  - Primary button: **Resume**
2. **Collapsed by default:**
  - Nested children (F1)
  - Timeline (F2)
  - Insights bullets (F6)
  - Raw event list (last 10)
3. **Expand affordance:** "Show details" / chevron per intent card.
4. **Visual style (adapt screenpipe spirit, not full clone):**
  - High whitespace (~40% empty)
  - 1px borders, sharp corners OK
  - No emoji in product chrome
  - State facts ("npm test failed") not marketing copy
5. **Never show raw JSON to user.**

### Acceptance

- First paint shows ≤3 intents with summaries only
- Expand reveals insights + timeline slice
- Demo video readable at 1080p

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md task C-SP1 and screenpipe DESIGN.md philosophy section.
Refactor Yesterday view for progressive disclosure: summary first, details on expand.
Integrate stats from B-P1 enrich API. No raw JSON visible.
```

---

## C-SP2: First-Run Onboarding Wizard

**Inspired by:** screenpipe onboarding + permissions flow

**Goal:** Download-and-play: user goes from install → capturing in <5 minutes.

### Steps (wizard screens)


| Step | Action                                                 | Verifies                 |
| ---- | ------------------------------------------------------ | ------------------------ |
| 1    | Welcome + privacy summary (link local PRIVACY.md)      | —                        |
| 2    | Run / show `intent-osctl enable`                       | systemd active           |
| 3    | Install VS Code extension (.vsix)                      | vscode events in status  |
| 4    | Load Firefox extension unpacked                        | firefox events in status |
| 5    | Enable shell hook (`intent-osctl shell enable`)        | shell events in status   |
| 6    | Add workspace (`intent-osctl workspace add ~/project`) | optional filesystem      |
| 7    | Done → open Yesterday view                             | health-check green       |


Poll `GET http://127.0.0.1:9477/v1/status` between steps (A-SP7).

### Acceptance

- Wizard completes on fresh Ubuntu with green status for vscode + firefox + shell
- Skip allowed per step with "fix later" link to TROUBLESHOOTING

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md task C-SP2.
Build first-run onboarding wizard polling /v1/status after each step.
Wire to intent-osctl commands or show copy-paste instructions.
```

---

## C-SP3: Intent Search UI (Ctrl+K)

**Inspired by:** screenpipe search UX + B-SP3 FTS

**Depends on:** B-P4 search API

### Tasks

1. Global shortcut Ctrl+K / Cmd+K opens modal.
2. Debounced fetch `GET /intents/search?q=`.
3. Show label + date + snippet; Enter navigates to intent detail.
4. Empty state: "Search your work memory…"

### Acceptance

- [ ] Search "login" finds demo intent in <300ms perceived
- Esc closes modal

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md task C-SP3 and INTENT-OS-FEATURES-SPEC.md F7.
Implement Ctrl+K search modal wired to Role B search API.
```

---

## C-SP4: Source Health Panel

**Inspired by:** screenpipe health-check MCP tool

**Depends on:** A-SP7

### Tasks

1. Settings or sidebar panel "Sources" showing status from `/v1/status`.
2. Per source: icon, event count, last seen, healthy badge.
3. Link each unhealthy source to `docs/TROUBLESHOOTING.md` anchor.

### Acceptance

- unhealthy shell shows red badge when no shell events in 30 min
- Refresh button re-fetches status

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md task C-SP4.
Add Sources health panel to settings using GET /v1/status from Role A.
```

---

## C-SP7: Yesterday Digest Hero (P1)

**Inspired by:** screenpipe daily briefing / first screen after open

**Depends on:** B-SP7

### Tasks

1. On Yesterday view load, fetch `GET /intents/digest`.
2. Hero card above intent list: `headline` + `summary` (2–3 lines max).
3. CTA: "See details" scrolls to parent intent card.

### Acceptance

- Digest visible before expanding any intent
- Matches B digest text for golden fixture

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md C-SP7.
Add digest hero card on Yesterday view wired to GET /intents/digest.
```

---

## C-SP8: Copy Agent Context (P1)

**Inspired by:** screenpipe agent workflows — one-click context for external LLM

**Depends on:** B-SP8

### Tasks

1. On intent detail: **Copy context** button → fetch `GET /intents/{id}/context` → clipboard.
2. Toast: "Context copied — paste into Claude or Codex."
3. Optional: same action in MCP setup screen as demo hint.

### Acceptance

- Clipboard contains markdown with auth.tsx + npm test for golden fixture
- Works without MCP installed

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md C-SP8.
Add Copy context button on intent detail calling GET /intents/{id}/context.
```

---

## C-SP5: Pin Intent Button

**Inspired by:** screenpipe update-memory

**Depends on:** B-SP5

### Tasks

1. Pin icon on intent card → `POST /intents/:id/pin`.
2. "Pinned" section at top of Yesterday view or separate tab.
3. Unpin action.

### Acceptance

- Pin persists after pipeline recompute
- Pinned section visible on reload

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md task C-SP5.
Add pin/unpin UI wired to Role B pin API. Show Pinned section in app.
```

---

## C-SP6: MCP Setup Screen (Optional)

**Inspired by:** screenpipe Settings → Connections

**Depends on:** B-SP4

### Tasks

1. Settings page shows Claude Desktop JSON snippet for `intent-os-mcp`.
2. Copy button + "Test connection" calling MCP health-check.
3. Example prompts: "What was I working on yesterday?", "Resume my login feature work.", "Summarize yesterday's digest."

### Acceptance

- Copy produces valid JSON with correct paths
- Test button shows success when MCP + servers running

### Codex prompt

```
Read INTENT-OS-SCREENPIPE-BORROWS.md task C-SP6.
Add MCP setup section to settings with copyable Claude config and connection test.
```

---

# 3. Cross-Role Integration Checklist

Before demo recording, all roles verify:


| Check                                           | A         | B               | C                  |
| ----------------------------------------------- | --------- | --------------- | ------------------ |
| Fixture ≥26 events                              | ✅         | consumes replay | fixture for UI dev |
| Pipeline produces intents with stats + insights | —         | ✅ B-P1–P3       | display only (C)   |
| Resume opens file + firefox + terminal          | ✅         | payload         | button             |
| Search finds demo intent (`login`)              | —         | ✅               | ✅                  |
| Digest + agent context                          | —         | ✅ B-SP7/8        | ✅ C-SP7/8          |
| MCP list-intents works                          | server up | ✅               | optional UI        |
| Onboarding green status                         | ✅         | —               | ✅                  |
| Notification on login                           | ✅ script  | top intent      | —                  |


---

# 4. Sprint Schedule (Full Team Track)


| Day    | A (Pranav)                 | B (Saloni & Mokshita)                          | C (Pranav — UX)                                    |
| ------ | -------------------------- | ---------------------------------------------- | -------------------------------------------------- |
| **D1** | A-SP5 fixture, A-SP4 JSONL | **B-P0** contract, **B-P1** start              | **C-BOOT1** scaffold + demo-intents fixture        |
| **D2** | A-SP7 status, fixture polish | **B-P1** golden tree, **B-P2** API + export    | **C-BOOT2** wire API, **C-F5** stats display       |
| **D3** | A-SP2, A-SP3 idle events   | **B-P3** LLM + enrich, **B-SP7/8** digest/context | **C-F2** timeline, **C-F1**, **C-SP1**, **C-SP7** |
| **D4** | integration fixes          | **B-P4** search, **B-SP4** MCP                 | **C-F6**, **C-SP3**, **C-F9**, **C-SP8**         |
| **D5** | `.deb`, record demo day    | **B-P5** F11/F10 gated                         | **C-SP2**, **C-DEMO1**, demo video                 |


**Windows-only (B):** replay `day.json` via `POST /pipeline/run-replay` — no Ubuntu required for Saloni & Mokshita.

**P2 stretch:** A-SP6/A-SP8 · B-SP5/B-SP6 · C-SP4–C-SP6, C-F10/F11 · A-SP1 dev/prod (post-hackathon)

---

# 5. Demo Script (90s — Building Login Feature)

**Act 1 — Capture (15s):** Quick cuts — edit `auth.tsx`, Google MDN for JWT, run `npm test` → red failure.

**Act 2 — Infer (25s):** Open Intent OS → digest hero: *"Building Login Feature"*. Expand nested tree: *Edit Auth Component* / *Fix Failing Tests*. Show insights: failed npm test, JWT research.

**Act 3 — Resume (20s):** Click **Resume** → VS Code opens `auth.tsx`, Firefox on MDN tab, terminal in `taskflow-app`.

**Act 4 — Agent-native (15s, optional):** Claude + MCP: *"What was I doing yesterday?"* → `daily-digest` or `list-intents` → *"Copy context"* / *"Resume it"*.

**Act 5 — Search (10s):** Ctrl+K → type `login` → jump to intent.

This story is easier to film and explain than infrastructure/Terraform.

---

# 5b. Demo Script Additions (Screenpipe-Inspired MCP)

Add to 90s video (optional 15s):

1. Open Claude with intent-os MCP: *"What was I working on yesterday?"*
2. Claude calls `list-intents` → reads summary → user says *"Resume it"*
3. Claude calls `resume-intent` OR user clicks Resume in app (show both if time)

This positions Intent OS as **agent-native context**, not just a GUI.

---

# 6. Explicit Non-Tasks (Do Not Assign to Codex)

- Dev vs prod data dirs / dual DB instances (post-hackathon)
- Background redaction worker (sync redact at ingest is enough)
- Screen recording / OCR pipeline
- Audio capture / transcription
- screenpipe Rust crate imports
- Cloud sync / team devices API
- Full pipes scheduler (cron markdown agents)
- GPU/onnx PII models
- macOS/Windows capture ports
- `device_id` / multi-user schema

---

# 7. Quick Reference — Task IDs

```
ROLE A (Pranav):
  A-SP1 SKIP (dev/prod)     A-SP2 URL blocklist      A-SP3 activity/idle events
  A-SP4 JSONL logs          A-SP5 login-feature fixture A-SP6 AT-SPI2 stretch
  A-SP7 status/onboard      A-SP8 redaction SKIP

ROLE B (Saloni & Mokshita) — see school/role-b.md:
  B-P0 contract             B-P1 deterministic core   B-P2 persistence + API
  B-P3 semantic quality     B-P4 search (F7)          B-P5 F10/F11/F12 gated
  B-SP4 MCP                 B-SP7 daily digest        B-SP8 agent context
  B-SP5 pins (stretch)      B-SP6 idle sessionize

ROLE C (Pranav — UX only):
  C-BOOT1 scaffold          C-BOOT2 wire Role B API
  C-F1 nested tree          C-F2 timeline             C-F3 greeting/empty
  C-F5 stats display        C-F6 insights display     C-F8 tag chips
  C-F9 Resume+Continue      C-F10/F11 display banners C-F12 TODO display
  C-DEMO1 demo polish
  C-SP1 progressive UI      C-SP2 onboarding          C-SP3 Ctrl+K search
  C-SP7 digest hero         C-SP8 copy context        C-SP4 health panel
  C-SP5 pin UI              C-SP6 MCP setup copy
```

**Start here:** B → `school/role-b.md` B-P0 · C → C-BOOT1 · Integration → B-P2 + C-BOOT2

---

*Role B: `school/role-b.md` wins on architecture/API. This doc wins on screenpipe borrow tasks. Feature spec F1–F12 applies via B phases.*
