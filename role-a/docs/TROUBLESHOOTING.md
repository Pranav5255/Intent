# Troubleshooting Role A

| Symptom | Check | Resolution |
| --- | --- | --- |
| No desktop focus events | `echo $XDG_SESSION_TYPE` | Use **Ubuntu on Xorg**. Wayland runs in degraded mode because X11 APIs are unavailable. |
| Firefox events missing | `about:policies` and `about:addons` | Confirm the signed Intent OS companion is installed and enabled. Existing managed Firefox policies are intentionally never overwritten. |
| VS Code/Cursor events missing | `code --list-extensions` or `cursor --list-extensions` | Run `intent-osctl vscode install` or `intent-osctl cursor install`; both use the VSIX bundled in the package. |
| Workspace events missing | `intent-osctl workspace list` | Add an approved workspace, then inspect `systemctl --user status intent-os-workspace-watch.service`. |
| Shell events missing | inspect `.bashrc`/`.zshrc` | Run `intent-osctl shell enable --shell <bash|zsh>` and open a new interactive shell. |
| Server unavailable | `systemctl --user status intent-os-server.service` | Run `intent-osctl enable`, then `intent-osctl status`. |
