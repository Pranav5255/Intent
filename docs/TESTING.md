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
| `package-macos` | macOS | Builds the daemon and `Intent.app`, runs the user-behaviour scenarios and the shell hooks, checks that the macOS collectors attach, installs the Homebrew formula and cask from the checkout ([`docs/MACOS.md`](MACOS.md)) |
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

## User-behaviour scenarios

[`dev/lima/scenarios.sh`](../dev/lima/scenarios.sh) acts like a user, a hostile web page
or a broken environment, and checks what Intent must do. It starts its own `intentd` on
`127.0.0.1:9478` with a throwaway home, so the installed service and its data are not
touched. Run it in the Lima VM or on a Mac (CI runs it on macOS too):

```bash
dev/lima/scenarios.sh                        # every scenario
dev/lima/scenarios.sh forget search          # scenarios whose name contains a word
INTENTD=~/target/release/intentd dev/lima/scenarios.sh   # a local build
SCENARIO_LLM=1 dev/lima/scenarios.sh search  # keep the configured model and embedder
```

| Scenario | The user (or the environment) |
|---|---|
| `baseline` | works a normal day; the day must have intents |
| `cross-site`, `dns-rebinding` | visits a web page that posts fake events or reads the config through a rebound hostname |
| `forget-day`, `forget-project`, `forget-race` | forgets a day or a project, also while the pipeline runs; nothing may come back |
| `retention` | sets a retention policy; old intents, search text and vectors go too |
| `timestamps`, `pause` | sends impossible timestamps or pause lengths |
| `shell-secrets`, `text-secrets` | types secrets in a shell, an editor, a web form or a private window |
| `blocklist`, `workspace`, `workspace-recreate` | makes a typo in the blocklist, approves `~` or `/`, deletes and re-clones a workspace |
| `search` | searches for `*`, `-`, `a..b`, a long unicode string or an unknown word |
| `stable-ids` | keeps an intent open while new activity arrives |
| `restart` | restarts the daemon while events arrive |

### The app, driven with Reticle

[`dev/lima/scenarios-app.sh`](../dev/lima/scenarios-app.sh) runs the app from source in the
VM with the Reticle SDK (dev build only; the packages are installed with `--no-save`).
Start `npx @reticlehq/server serve` on the Mac and forward its port into the VM, as the
script header shows. Then drive these journeys and assert the end state:

| Journey | End state |
|---|---|
| Today is open while new threads are captured | the new threads appear without a reload |
| Forget a day from the drawer | the threads leave Today at once and stay gone after the next pipeline run |
| Search for `axum..routing` | a search request is sent and results or "Nothing matched" show |
| Search while `intentd` is stopped | an error shows, not "Nothing matched" |
| Double-click **Resume** | one restore request |
| First launch with `intentd` stopped, then started | **Start capturing** becomes available |

## Locally

```bash
git config core.hooksPath .githooks   # once per clone
dev/ci/check.sh                       # the gates that don't need a Linux desktop
dev/ci/check.sh --fast                # format, hygiene and clippy
```

The pre-commit hook checks hygiene, formatting and shell scripts on staged files; the
pre-push hook runs `dev/ci/check.sh`. For the `.deb`, end-to-end and budget gates, use
the Lima VM (see the README) or let CI run them.
