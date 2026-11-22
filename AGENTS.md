# AGENTS.md — Intent

The rules for every agent that changes this repository. Claude Code, Codex, Cursor, Gemini,
Copilot and OpenCode read this file. `CLAUDE.md` imports it.

## What Intent is

Intent is a local-first Ubuntu desktop companion. It records what the user works on
(focused windows, shell commands, files in approved workspaces, browser tabs, editor
activity). It groups that activity into *intents*, and it lets the user resume one: reopen
its files, tabs and terminal. All data stays on the machine. An LLM is optional and local
by default.

Architecture, the HTTP API and the auth table: [`docs/REWRITE.md`](docs/REWRITE.md).
Test gates: [`docs/TESTING.md`](docs/TESTING.md).

## Layout

```
crates/intent-types       wire and pipeline contracts (serde); the frozen contract
crates/intent-store       SQLite (WAL) + FTS5 + sqlite-vec; single writer, migrations, 1.x importer; sync
crates/intent-capture     validation, redaction, blocklist, consent configs, retention, restore, XDG paths; pure, sync
crates/intent-llm         provider clients, structured-output harness, embeddings, refinement, labeler, copilot
crates/intent-engine      deterministic pipeline, digest, resume selection, current intent
crates/intent-collectors  activity sources: focus and idle (X11, GNOME Wayland, macOS), filesystem watcher
crates/intentd            the daemon: axum on 127.0.0.1:9477, ingest writer, scheduler, retention, collectors
crates/intentctl          CLI and shell-hook sender
app/                      Tauri 2 desktop app: React + TypeScript in src/, Rust in src-tauri/ (own Cargo tree)
shell/                    bash and zsh hooks, and their test driver
gnome-extension/          reports the focused window on GNOME Wayland
integrations/             Firefox, Chrome and VS Code companions (Chrome reuses the Firefox scripts)
packaging/                .deb build, systemd user unit, maintainer scripts, Homebrew cask generator
Formula/                  Homebrew formula; the repository is its own tap
dev/ci/                   gate scripts: check.sh, tree-hygiene.sh, budgets.sh
dev/lima/                 Ubuntu test VM, end-to-end and measurement scripts
docs/                     architecture, gates, screenshots
```

## Boundaries

- **`intent-types` is the contract.** The daemon, the CLI, the app and the companions all
  depend on its shapes. Change a type only on purpose, and update every consumer in the
  same change.
- **Dependencies point one way.** `intentd` wires everything. `intent-engine` uses
  `intent-llm` only through the traits in `intent-llm/src/traits.rs`. The other library
  crates depend only on `intent-types`. Do not add an edge between library crates.
- **`intent-store` and `intent-capture` stay sync.** No tokio in them.
- **Platform code is behind `#[cfg(target_os = "linux")]` or `#[cfg(target_os = "macos")]`.**
  The workspace must build, test and run on both. See `docs/MACOS.md`.
- **`app/src-tauri` is a separate Cargo tree.** It is excluded from the workspace and has its
  own `Cargo.lock`. Run its checks from `app/src-tauri`.

## Privacy and security

Intent records a person's activity. A leak here is the worst defect this project can ship.

1. `intentd` listens on loopback only. Do not bind to another address outside tests.
2. Captured data is never readable without the bearer token. A new route requires the
   token unless a companion or the shell hook needs it without one. Add every new route to
   the route table in `docs/REWRITE.md`, with its token column.
3. Every event goes through one ingest path: validation, pause check, admission (consent,
   workspace approval, redaction, blocklist), then the writer queue. Do not write events
   to the store any other way.
4. Do not log captured content (titles, URLs, commands, paths, file contents) at `info` or
   above. Do not log the token or LLM credentials at any level.
5. Nothing leaves the machine unless the user configured a cloud provider. Do not add
   telemetry, update checks or any other network call.
6. Keep CORS to the origins in `docs/REWRITE.md`. Dev origins are for debug builds only.
7. Secret files (the token, `llm.toml`) are mode `0600`.

## Rules

1. Write the test first. A bug fix starts with a test that shows the bug. Prove that the
   test fails without the fix.
2. Pass the clock in. Engine and capture logic take the current time as a parameter and
   never read the wall clock. `Instant` for measuring elapsed time is fine.
3. No `unwrap()` or `expect()` on data from outside the process (HTTP bodies, files, the
   database, LLM output). Return an error. Library crates use `thiserror`; `intentd` and
   `intentctl` use `anyhow`.
4. Do not add an `allow` for a lint. Change the code so that clippy passes with
   `-D warnings`.
5. A schema change is a new entry in `MIGRATIONS` in `intent-store/src/schema.rs`. Do not
   edit a migration that has shipped.
6. Engine behaviour changes update the golden fixtures in
   `crates/intent-engine/tests/fixtures` in the same change, with the reason in the commit.
7. An API change updates the app mock (`app/src/lib/mock.ts`) in the same change, so that
   `npm run dev:mock` and the screenshots still show real shapes.
8. The daemon has budgets (idle RSS, idle CPU, ingest rate, pipeline time, peak RSS) in
   `dev/ci/budgets.sh`. Raise a budget only with a measurement that explains it, in the
   same change.
9. Do not add busy polling. Background work runs on the 60 s housekeeping timer or on an
   event.
10. Keep a source file under about 1,000 lines. Split by responsibility, not by size.
11. Do not assert a duration in a unit test. It fails only under load. Use the budgets
    script for performance.
12. Write in short, plain sentences: docs, comments, commits, PR text and error messages.
    Use the active voice. Comments explain why, not what.

## Before you commit

```bash
git config core.hooksPath .githooks   # once per clone
dev/ci/check.sh --fast                # format, hygiene, clippy
dev/ci/check.sh                       # also tests, shell hooks, app typecheck, cargo deny
```

`cargo fmt --all` fixes workspace formatting. Run `cargo fmt` in `app/src-tauri` for the
app crate. The pre-commit hook checks staged files. The pre-push hook runs `check.sh`.

## Which gate to run

`dev/ci/check.sh` covers every change. Some changes need one more check.

| If you changed | Also run |
| --- | --- |
| the app frontend | `cd app && npm run typecheck && npm run build:mock` |
| the app's Rust crate | `cd app/src-tauri && cargo clippy --all-targets --locked && cargo test --locked` |
| collectors, packaging, the systemd unit, or anything only Linux runs | `dev/lima/e2e.sh` in the Lima VM |
| the app against the daemon, the tray, the palette or close-to-tray | `dev/lima/e2e-app.sh` in the Lima VM |
| macOS collectors, launchd, Resume on macOS, or the Homebrew formula | the steps of the `package-macos` CI job, see `docs/MACOS.md` |
| ingest, the writer, the pipeline or the scheduler | `dev/ci/budgets.sh` in the Lima VM |
| a dependency | `cargo deny --locked check` for the tree you changed |

The README shows how to start the Lima VM. CI runs every gate on every pull request, so
you can also push and read the result.

## Commits and pull requests

1. `main` accepts changes only through a pull request with the `gate` check green. Do not
   push to `main`. Branch from `main`, and rebase on `origin/main` to update.
2. Use Conventional Commits: `fix(intentd): reject a pause in the past`.
3. Do not add `Co-authored-by:` trailers or tool attribution lines to commits or PRs. Do
   not change the git identity.
4. One concern per pull request. Say in the PR how you tested it and that the new test
   fails without the change.
5. A change a user can notice adds a line under `## [Unreleased]` in `CHANGELOG.md`, in
   the same pull request. Use the sections of [Keep a Changelog](https://keepachangelog.com/en/1.1.0/):
   Added, Changed, Deprecated, Removed, Fixed, Security. Write what changed for the user,
   not the commit message, and link the pull request. Tests, CI and refactors need no
   entry.
6. Do not commit build output, databases, `.deb`/`.xpi`/`.vsix` files, env files or files
   over 1 MB. `dev/ci/tree-hygiene.sh` fails on them.
