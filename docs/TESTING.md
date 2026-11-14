# Test gates

Nothing reaches `main` without passing **`gate`**, the one required status check. It
aggregates every job in [`ci.yml`](../.github/workflows/ci.yml) and fails if any of them
failed, was cancelled or was skipped. `main` only accepts changes through a pull request
with `gate` green; force-pushes and deletion are blocked, with no bypass.

## What `gate` runs

| Job | Runner | Checks |
|---|---|---|
| `lint` | Ubuntu | `rustfmt` (workspace and app), `shellcheck` on every script and hook, tree hygiene (`dev/ci/tree-hygiene.sh`: no env files, keys, credentials, build output or files over 1 MB) |
| `rust` | Ubuntu amd64 + arm64 | `clippy -D warnings` with the Linux collectors compiled in; the full test suite, in debug and release |
| `rust-macos` | macOS | clippy and tests, so the workspace keeps building off Linux |
| `msrv` | Ubuntu | `cargo check` on the declared `rust-version` |
| `app` | Ubuntu | `npm ci`, TypeScript typecheck, frontend build (real and mock), clippy and tests for the Tauri crate |
| `shell-hooks` | Ubuntu | `shell/test_hooks.sh` drives real interactive bash and zsh sessions |
| `audit` | Ubuntu | `cargo deny` ([`deny.toml`](../deny.toml): advisories, yanked crates, licenses, sources) for both Rust trees; `npm audit` for the app and companions |
| `package` | Ubuntu amd64 + arm64 | Builds the `.deb` (daemon, CLI, app), installs it on a clean runner and runs it for real, see below |

### `package`: the product, installed and used

1. `packaging/build-deb.sh` with `INTENT_BUILD_APP=1`.
2. [`dev/lima/e2e.sh`](../dev/lima/e2e.sh): install, `intentctl enable` under a systemd
   user session, then real activity: window focus switches under Xvfb + openbox, shell
   commands through the bash hook, file edits in an approved workspace, Firefox and
   VS Code payloads. Then the pipeline, search, digest and resume selection, checked
   through the API.
3. [`dev/ci/budgets.sh`](../dev/ci/budgets.sh): resource budgets. Fails when the daemon
   is heavier or slower than allowed:

   | Budget | Limit |
   |---|---|
   | idle RSS | ≤ 40 MB |
   | idle CPU over 30 s | ≤ 1 % |
   | ingest, 5000 events from 32 clients | ≥ 1000 events/s, all `201` |
   | forced pipeline over 2000 events | ≤ 1500 ms |
   | peak RSS after ingest and pipeline | ≤ 80 MB |

4. [`dev/lima/e2e-app.sh`](../dev/lima/e2e-app.sh): the installed app against the real
   daemon: real data on every screen, no unauthorised calls, the palette shortcut,
   pause from the palette, tray registration, close-to-tray. Screenshots and the `.deb`
   are uploaded as run artifacts.

Raise a budget only with a measured reason in the same change.

## Outside `gate`

- **CodeQL** ([`codeql.yml`](../.github/workflows/codeql.yml)): Rust, TypeScript and
  workflow analysis on every change and weekly. Findings go to the Security tab.
- **OpenSSF Scorecard** ([`scorecard.yml`](../.github/workflows/scorecard.yml)): weekly
  supply-chain review.
- **Nightly CI** on `main` with the newest stable Rust and current advisories, so a new
  lint, a yanked crate or a fresh RustSec advisory shows up without waiting for a change.
- **Dependabot** ([`dependabot.yml`](../.github/dependabot.yml)): weekly grouped updates
  for Cargo, npm and Actions, with a 7-day cooldown; every update passes `gate` like
  any other change.

All Actions are pinned to commit SHAs and workflows run with read-only tokens.

## Locally

```bash
git config core.hooksPath .githooks   # once per clone
dev/ci/check.sh                       # the gates that don't need a Linux desktop
dev/ci/check.sh --fast                # format, hygiene and clippy
```

The pre-commit hook checks hygiene, formatting and shell scripts on staged files; the
pre-push hook runs `dev/ci/check.sh`. For the `.deb`, end-to-end and budget gates, use
the Lima VM (see the README) or let CI run them.
