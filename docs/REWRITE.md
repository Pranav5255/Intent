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

**Auth.** Routes that change state or launch programs (restore, settings, purges, memory
deletion, pause, PUT config) require `Authorization: Bearer <token>`. The token is generated at
first start into `$XDG_RUNTIME_DIR/intent/token` (fallback `~/.local/share/intent/token`),
mode 0600. Ingest (`POST /v1/event`) and read-only config stay open to local companions.

### Data

- One SQLite database: `$XDG_DATA_HOME/intent/intent.db` (WAL). Tables cover raw events,
  day stats, intents, pipeline runs, metadata, FTS5 search, embeddings (sqlite-vec) and
  user facts. A one-time importer reads the 1.x `events.db` / `intents.db` if present.
- Config stays in `$XDG_CONFIG_HOME/intent/` with the 1.x file names
  (`detailed-capture.json`, `workspaces.json`, `filesystem-capture.json`,
  `retention-policy.json`, `blocked-domains.yaml`). LLM settings move to `llm.toml` (0600);
  a 1.x `llm.env` is imported once.
- No mem0. Semantic memory = FTS5 + sqlite-vec over intent summaries + a small `facts` table.

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
