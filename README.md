# Intent

Local-first Ubuntu desktop companion for recalling and resuming recent work. Intent
watches what you do (focused windows, shell commands, files in approved workspaces,
browser tabs, editor activity), groups it into *intents* — threads of work — and lets you
pick one up again: reopen its files, tabs and terminal.

Everything runs and stays on your machine. An LLM is optional and local by default
(Ollama, llama.cpp or any OpenAI-compatible server); cloud providers are opt-in.

## Layout

| Path | What |
|------|------|
| `crates/intentd` | The daemon: HTTP API on `127.0.0.1:9477`, ingest, collectors, scheduler, retention |
| `crates/intentctl` | CLI and shell-hook sender |
| `crates/intent-types` | Shared wire and pipeline types |
| `crates/intent-store` | SQLite (WAL) + FTS5 + sqlite-vec |
| `crates/intent-capture` | Validation, redaction, consent configs, restore |
| `crates/intent-engine` | Sessionize → cluster → refine → label pipeline, digest, resume |
| `crates/intent-llm` | Local-first LLM harness, embeddings, copilot |
| `crates/intent-collectors` | X11 / GNOME Wayland focus, idle, filesystem watcher |
| `app/` | Tauri 2 desktop app (React + TypeScript) |
| `shell/` | bash and zsh hooks |
| `gnome-extension/` | Reports the focused window on GNOME Wayland |
| `integrations/` | Firefox and VS Code companions |
| `packaging/` | `.deb` build, systemd user unit |
| `dev/lima/` | Ubuntu test VM, end-to-end and measurement scripts |

Architecture, API and auth details: [`docs/REWRITE.md`](docs/REWRITE.md).

## Develop

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# UI against mock data (no daemon needed), http://localhost:1420
cd app && npm ci && npm run dev:mock
```

The workspace builds and tests on macOS; Linux-only collectors are behind
`cfg(target_os = "linux")`.

## Build and test on Ubuntu

The `.deb` is built on the target architecture. On a Mac, use the Lima VM:

```bash
limactl start --name=intent dev/lima/intent-dev.yaml
limactl shell intent -- bash -lc 'INTENT_BUILD_APP=1 packaging/build-deb.sh --output ~/dist/intent_2.0.0_arm64.deb'
limactl shell intent -- bash -lc 'dev/lima/e2e.sh && dev/lima/e2e-app.sh'
```

Optional companions are bundled when built first:

```bash
(cd integrations/firefox-extension && npm run build)
(cd integrations/vscode-extension && npm ci && npm run package)
```

## Install and use

```bash
sudo apt install ./intent_2.0.0_amd64.deb
intentctl enable                  # start the user service and shell hook
intentctl workspace add ~/src     # allow file activity under a directory
intent                            # open the app (Super+Space opens the palette)
```

`intentctl pause 30m`, `intentctl status`, `intentctl run` and `intentctl uninstall`
cover the rest.
