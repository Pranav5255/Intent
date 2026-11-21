# Intent

Local-first desktop companion for Ubuntu and macOS, for recalling and resuming recent work. Intent
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
| `crates/intent-collectors` | Focus and idle (X11, GNOME Wayland, macOS), filesystem watcher |
| `app/` | Tauri 2 desktop app (React + TypeScript) |
| `shell/` | bash and zsh hooks |
| `gnome-extension/` | Reports the focused window on GNOME Wayland |
| `integrations/` | Firefox and VS Code companions |
| `packaging/` | `.deb` build, systemd user unit, Homebrew cask generator |
| `Formula/` | Homebrew formula (the repository is its own tap) |
| `dev/lima/` | Ubuntu test VM, end-to-end and measurement scripts |

Architecture, API and auth details: [`docs/REWRITE.md`](docs/REWRITE.md).
Test gates: [`docs/TESTING.md`](docs/TESTING.md).

## Develop

```bash
git config core.hooksPath .githooks   # pre-commit and pre-push checks
dev/ci/check.sh                       # local gates: fmt, clippy, tests, hooks, typecheck

# UI against mock data (no daemon needed), http://localhost:1420
cd app && npm ci && npm run dev:mock
```

The workspace builds, tests and runs on macOS too. Platform code is behind
`cfg(target_os = "linux")` or `cfg(target_os = "macos")`. See [`docs/MACOS.md`](docs/MACOS.md).

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

### macOS (Homebrew)

```bash
brew tap pranav5255/intent https://github.com/Pranav5255/Intent
brew install --HEAD intent        # daemon and CLI, built from source
intentctl enable                  # start at login (launchd) and add the shell hook
```

The app installs with `brew install --cask intent-app` once a release has the disk
images. Until then, build it with `cd app && npm ci && npx tauri build --bundles app`.
Details and limits: [`docs/MACOS.md`](docs/MACOS.md).
