# Install Intent OS on Ubuntu

## Prerequisites

- Ubuntu 24.04+ on the same architecture/Python ABI as the package build, with
  a GNOME **X11** session for foreground-window tracking.
- Firefox for browser metadata capture.
- VS Code is optional but recommended for editor-specific file context.

The unified package includes Role A, Role B, the Firefox companion, and the
VS Code/Cursor companion. Role B runs as a local user service on port `9478`
with bundled Python dependencies; the package never installs VS Code or Cursor.

## Build artifacts

Build the companions first:

```bash
cd integrations/firefox-extension && npm test && npm run build
cd ../vscode-extension && node --test tests/*.test.js
npx --yes @vscode/vsce package --out dist/intent-os-vscode.vsix
```

For a developer package, which does not modify Firefox policy because its XPI
is unsigned:

```bash
bash packaging/build-deb.sh --dev
```

For a release package, create the signed self-distributed XPI with AMO credentials. The signing script uses `web-ext sign --channel unlisted`, writes a checksum, and never prints the secret. The release package installs Firefox's `normal_installed` policy only when no administrator-managed policy already exists.

```bash
# Fill the ignored, owner-only file with a newly generated AMO credential pair.
nano .env
unset AMO_JWT_ISSUER AMO_JWT_SECRET  # ensures .env is used
bash scripts/sign_firefox.sh
INTENT_OS_FIREFOX_XPI="$(pwd)/integrations/firefox-extension/dist/intent-os-firefox-signed.xpi" \
  bash packaging/build-deb.sh
sudo apt install ./dist/intent-os_0.1.1_amd64.deb
```

## Per-user enablement

After installation, each user explicitly enables the local Role A capture
services and the Role B deterministic engine:

```bash
intent-osctl enable
intent-osctl vscode install       # if VS Code is installed
intent-osctl cursor install       # if Cursor is installed
intent-osctl workspace add ~/work/infra
intent-osctl shell enable --shell bash
intent-osctl status
```

`intent-osctl enable` starts the event API on `9477` and the Role B API on
`9478`, alongside the X11 tracker and workspace watcher. `intent-osctl disable`
stops and disables all user services. `intent-osctl shell disable --shell bash`
removes only the marked Intent OS block from the shell rc file.

## Local backend data

Packaged services keep their local backend data in `~/.local/share/intent-os`.
Role A exclusively owns the raw captured-event database at `events.db`; Role B
exclusively owns the derived-intent database at `intents.db`. The databases
remain separate: Role B accesses Role A through its local HTTP API and does not
open `events.db` directly.


## Detailed capture (opt-in)

Detailed capture is disabled until it is explicitly enabled. Add the workspace
first, then enable the server-side consent switches:

```bash
intent-osctl detailed editor enable
intent-osctl detailed browser enable
intent-osctl status
```

For VS Code or Cursor, also set **Intent OS: Detailed Capture** to true in the
editor settings (or add `"intentOS.detailedCapture": true` to settings JSON).
The extension sends detailed editor events only when both this editor setting
and the server-side editor consent are enabled.

The user-editable exclusions are stored in
`~/.config/intent-os/detailed-capture.json`. Detailed events are retained
locally until manually deleted:

```bash
intent-osctl purge-detailed
```

Every `intent-osctl export-day` result includes detailed events. Review the
export before sharing it, because it can include bounded inserted source text
and semantic browser action labels. Rebuild and re-sign the Firefox XPI after
changing the companion source before producing a release package.

## Diagnostics

```bash
echo "$XDG_SESSION_TYPE"          # x11 for desktop window titles
intent-osctl status               # source counts and last event timestamps
intent-osctl export-day --date 2026-07-13 > day.json
```

Firefox and VS Code are independent sources. If either companion is disabled,
the server and remaining collectors continue working; the workspace watcher
provides only saved-file fallback context.
