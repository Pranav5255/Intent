# Troubleshooting Role A

| Symptom | Check | Resolution |
| --- | --- | --- |
| No desktop focus events | `echo $XDG_SESSION_TYPE` | Use **Ubuntu on Xorg**. Wayland runs in degraded mode because X11 APIs are unavailable. |
| Firefox events missing | `about:policies` and `about:addons` | Confirm the signed Intent OS companion is installed and enabled. Existing managed Firefox policies are intentionally never overwritten. |
| VS Code/Cursor events missing | `code --list-extensions` or `cursor --list-extensions` | Run `intent-osctl vscode install` or `intent-osctl cursor install`; both use the VSIX bundled in the package. |
| Workspace events missing | `intent-osctl workspace list` | Add an approved workspace, then inspect `systemctl --user status intent-os-workspace-watch.service`. |
| Shell events missing | inspect `.bashrc`/`.zshrc` | Run `intent-osctl shell enable --shell <bash|zsh>` and open a new interactive shell. |
| Server unavailable | `systemctl --user status intent-os-server.service` | Run `intent-osctl enable`, then `intent-osctl status`. |

`intent-osctl status` also reports connector health. A source is healthy when it
has sent an event within the previous 30 minutes (set
`INTENT_OS_SOURCE_STALE_AFTER_SECONDS` to adjust this for development). For
VS Code, reinstall or enable the companion extension; for Firefox, re-enable
the companion in `about:addons`; for shell, re-run the shell integration and
open a new terminal; and for Linux focus events, use an Xorg session and check
`intent-os-x11-tracker.service`. The workspace watcher is healthy after an
approved workspace produces filesystem events.

Browser URLs on domains listed in `~/.config/intent-os/blocked-domains.yaml`
are recorded as `[blocked]` with no query string, action context, or target
label. Start from `config/blocked-domains.yaml.example`, then inspect the
active rules with `curl http://127.0.0.1:9477/v1/config`.
