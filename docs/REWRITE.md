# Intent 2.0 — Rust rewrite

Goal: one lightweight native daemon plus a small Tauri app, replacing 4–5 long-lived
Python processes, per-command Python forks and Electron.

## Runtime shape

| Process | Replaces | Notes |
|---|---|---|
| `intentd` (Rust, tokio) | capture server, engine API, scheduled ingest timer, X11 tracker, workspace watcher, tray | One `systemd --user` service. Target idle RSS < 40 MB, ~0% CPU. |
| `intent` (Tauri 2) | Electron overlay | Started on demand / autostart, owns the tray icon and global shortcut. |
| `intentctl` (Rust CLI) | `intentctl.py`, `intent-shell-send` | Also the shell-hook sender (`intentctl send-shell …`) — no Python per command. |
| GNOME Shell extension | (none — new) | Reports focused window over D-Bus on Wayland, where X11 tracking is blind. |

### HTTP

`intentd` listens on **127.0.0.1:9477** only. All 1.x capture routes keep their paths and
shapes (`/v1/event`, `/v1/events`, `/v1/export/day[/meta]`, `/v1/events/incremental`,
`/v1/detailed-capture/config`, `/v1/workspaces`, `/v1/filesystem-capture`,
`/v1/retention/*`, `/v1/restore`, `/v1/capture/pause`, `/v1/status`, `/v1/config`,
`/healthz`, plus the unprefixed aliases), so the Firefox and VS Code companions work
unchanged. The former engine routes (`/intents/*`, `/settings/llm`, `/copilot/*`,
`/resume/select`, `/pipeline/*`, `/v1/memory/*`) move onto the same port with the same paths.

**Auth.** Everything requires `Authorization: Bearer <token>` except the surface the
browser/editor companions and the shell hook use. The token is generated at first start into
`$XDG_RUNTIME_DIR/intent/token` (fallback `~/.local/share/intent/token`), mode 0600; the Tauri
app and `intentctl` read it.

| Route | Methods | Token | Notes |
|---|---|---|---|
| `/healthz` | GET | open | `{ok, version, pipeline_version, database}` |
| `/v1/event`, `/event` | POST | open | 201 stored · 200 duplicate id · 204 dropped (paused / no consent / workspace not approved) · 415 content type is not `application/json` · 422 invalid or rejected by redaction · 503 writer queue full / unavailable |
| `/v1/detailed-capture/config` | GET | open | read by the Firefox and VS Code companions |
| `/v1/config` | GET | open | blocked domains |
| `/v1/detailed-capture/config` | PUT | **token** | |
| `/v1/events`, `/events` · `/v1/events/incremental`, `/events/incremental` · `/v1/export/day[/meta]`, `/export/day[/meta]` | GET | **token** | captured data is never readable without the token |
| `/v1/workspaces` · `/v1/filesystem-capture` | GET, PUT | **token** | PUT re-configures the filesystem watcher live |
| `/v1/retention/policy` | GET, PUT | **token** | |
| `/v1/retention/preview`, `/retention/preview` | GET | **token** | |
| `/v1/retention/purge`, `/retention/purge` | POST | **token** | 409 unless `confirm: true` |
| `/v1/detailed-capture/purge` | POST | **token** | |
| `/v1/intelligence/preview-sample` | GET | **token** | |
| `/v1/restore`, `/restore` | POST | **token** | launches editor/browser/terminal |
| `/v1/capture/pause` | POST | **token** | `{paused, duration_seconds?}`; persisted with its resume time |
| `/v1/status`, `/status` | GET | **token** | 1.x shape + `collectors[]`, `activity`, `llm`, `database`, `paused_until`, `pipeline.last_run`, `retention` |
| `/intents/*`, `/resume/select`, `/copilot/*` | GET/POST | **token** | `/copilot/*` → 503 `copilot_not_configured` without a usable provider |
| `/settings/llm`, `/settings/llm/gemini-credentials` | GET, PUT / POST, DELETE | **token** | PUT rebuilds providers atomically |
| `/pipeline/run[?date]`, `/pipeline/run-replay`, `/pipeline/recompute?date` | POST | **token** | `run` defaults to today |
| `/v1/memory/date/{date}`, `/v1/memory/project/{project}` | DELETE | **token** | |
| any `OPTIONS` preflight | OPTIONS | open | |

**CORS.** Only `INTENT_FIREFOX_EXTENSION_ORIGIN` (the companion's `moz-extension://…` origin),
`tauri://localhost` and `http://tauri.localhost` are allowed (debug builds add the Tauri dev
server on `:1420`); Vite/localhost dev origins are never allowed in release builds.

**Browser guards.** CORS only hides responses, so three more checks run before auth:

- **Host.** Every request needs a `Host` of `127.0.0.1`, `localhost` or `[::1]`, with any
  port or none. Anything else gets 421. This stops DNS rebinding: a page that rebinds its own
  hostname to `127.0.0.1` still sends its own `Host`.
- **Origin.** A POST, PUT or DELETE that carries an `Origin` must come from a
  `moz-extension://` origin (a page cannot send that scheme), `tauri://localhost` or
  `http://tauri.localhost` (debug builds add the dev server on `:1420`). Anything else,
  `null` included, gets 403. Clients without an `Origin` (the shell hook, `intentctl`, the
  VS Code companion, the app's Rust side) pass.
- **Content type.** `POST /v1/event` and `/event` need `application/json` (parameters such as
  `charset` are fine) and answer 415 otherwise. A page can send `text/plain` without a
  preflight; JSON forces one, which CORS refuses.

### Background work inside `intentd`

All on one 60 s housekeeping timer plus an event-driven embedding task (no busy polling):

- **Scheduler** (replaces `intent-pipeline.timer`): when new events arrived since the last run
  and the user has been idle ≥ 2 min, or 30 min passed, run today's pipeline; skip when the
  day's `content_hash` is unchanged. Yesterday is finalized once after midnight. The outcome is
  stored as `pipeline_last_run` metadata and shown in `/v1/status`.
- **Retention**: the saved policy is applied ~30 s after start and then daily.
- **Pause expiry**: a timed pause resumes on time, also across restarts.
- **Embedding backfill**: root intents without a vector are embedded after every pipeline run
  (and every 15 min), with the configured embedder (offline hashing embedder by default).
- **"Continue yesterday?"** (Linux): one desktop notification per morning while the user is
  active and capture is not paused; its action runs `intent --preview <intent_id>`.
  `INTENT_NOTIFICATIONS=0` disables it.

Logging goes to stderr (journald under systemd; no log files). `INTENT_LOG` / `RUST_LOG` set the
filter. `INTENT_BIND` overrides the loopback listen address (tests).

### Data

- One SQLite database: `$XDG_DATA_HOME/intent/intent.db` (WAL). Tables cover raw events,
  day stats, intents, pipeline runs, metadata, FTS5 search, embeddings (sqlite-vec) and
  user facts. A one-time importer reads the 1.x `events.db` / `intents.db` if present.
- Config stays in `$XDG_CONFIG_HOME/intent/` with the 1.x file names
  (`detailed-capture.json`, `workspaces.json`, `filesystem-capture.json`,
  `retention-policy.json`, `blocked-domains.yaml`). LLM settings move to `llm.toml` (0600);
  a 1.x `llm.env` is imported once.
- No mem0. Semantic memory = FTS5 + sqlite-vec over intent summaries + a small `facts` table.

### Ingest path

HTTP (`POST /v1/event`) and the in-process collectors share one path: validation
(`intent_capture::validate_event`) → persisted pause check → `ConfigStore::admission_policy()`
+ `admit` (consent, workspace approval, redaction, blocklist) → bounded writer queue (1024) →
one SQLite transaction per batch (≤ 100 events, ≤ 25 ms). Unlike 1.x the batch closes as soon
as no new event arrives for 2 ms, so a client waiting for its 201 is no longer capped at one
event per 25 ms. Writer metrics (accepted, inserted, duplicates, drops by reason, batches,
failures, queue high-water mark, average wait/write ms) are in `/v1/status.ingestion`.

## Packaging and verification

- `packaging/build-deb.sh` (run on the Ubuntu build host / Lima VM) builds release `intentd` +
  `intentctl` and assembles `intent_2.0.0_<arch>.deb` with `dpkg-deb`: binaries in `/usr/bin`,
  `intentd.service` in `/usr/lib/systemd/user`, shell hooks in `/usr/share/intent/shell`, the
  GNOME extension, the companions found under `integrations/*/dist` (skipped with a
  warning otherwise) and the Firefox policy logic ported from 1.x (policy only installed for a
  release build with a signed XPI, never over an administrator-managed policy). Depends:
  `libc6, libgcc-s1` only. With `INTENT_BUILD_APP=1` it also builds the Tauri app
  (`npm ci` + `tauri build --no-bundle`) and ships `/usr/lib/intent/intent-app` behind the
  `/usr/bin/intent` launcher, `io.github.pranav5255.Intent.desktop` and hicolor icons; Depends
  then come from `dpkg-shlibdeps` plus `libayatana-appindicator3-1 | libappindicator3-1`
  (dlopen()ed for the tray). No system-wide XDG autostart: "Launch at login" in the app writes
  the per-user entry. The launcher runs `intentctl enable` only when the service is not enabled
  yet (1.x did it every launch). The app disables WebKit accelerated compositing unless
  `WEBKIT_DISABLE_COMPOSITING_MODE` is set (≈ 320 MB vs 680 MB PSS idle in the VM).
- `dev/lima/e2e.sh` installs the .deb in the VM, drives an Xvfb + openbox session (xdotool focus
  switches, an interactive bash with the hook, file writes in an approved workspace, companion
  payloads) and checks events, intents, search, digest and status through the API.
- `dev/lima/e2e-app.sh` (after `e2e.sh`, or it runs it) drives the installed app in the same
  session: main window views, `intent --preview <id>`, Super+Space palette toggling, pause from
  the palette, tray registration (stalonetray), a loopback capture proving no 401s, and idle
  RSS/PSS. Screenshots: `docs/screenshots/vm-*.png`.
- `dev/lima/measure.sh idle|ingest|pipeline` reports RSS/PSS, idle CPU, ingest events/s
  (`crates/intentd/examples/ingest_load.rs`) and pipeline latency.

## Crates and ownership

| Crate | Responsibility | Depends on |
|---|---|---|
| `intent-types` | Wire + pipeline contracts (serde). **Frozen contract** — change only with care. | serde |
| `intent-store` | rusqlite (bundled) + FTS5 + sqlite-vec. Single writer thread, read pool, migrations, 1.x importer. Sync API. | types |
| `intent-capture` | Event validation, redaction (incl. shell), URL blocklist, consent configs, retention policy, restore launcher, XDG paths. Pure / sync. | types |
| `intent-llm` | Provider clients, structured-output harness, embeddings, semantic refinement, LLM labeler, copilot loop. Exposes the traits in `traits.rs`. | types |
| `intent-engine` | Deterministic pipeline: normalize → sessionize → cluster → (refine via trait) → label (via trait) → enrich/resume → tree. Plus digest, context, prediction, resume selection, current intent. Pure + async orchestration. | types, llm (traits only) |
| `intent-collectors` | Linux activity sources: X11 focus (x11rb), idle, GNOME/Wayland focus over D-Bus, filesystem watcher (notify). Emit `RawEvent`s into a channel. | types |
| `intentd` | Wires everything: axum server, ingest writer, scheduler, retention, collectors. | all |
| `intentctl` | CLI + shell sender. | types, capture |

## Engineering rules

- Edition 2024, stable Rust. `cargo clippy --workspace -- -D warnings` and `cargo test --workspace` stay green.
- Port behaviour, not bugs: the 1.x bugs found during the audit are fixed (see git history / PR).
- Golden tests: engine behaviour is checked against fixtures under `crates/intent-engine/tests/fixtures`.
- Linux-only code is behind `#[cfg(target_os = "linux")]` so the workspace still builds and tests on macOS.
