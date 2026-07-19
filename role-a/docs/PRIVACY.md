# Intent OS Role A privacy boundary

All Role A data stays on the local machine in
`~/.local/share/intent-os/events.db` until the user exports a `day.json` file.
Detailed events are retained indefinitely and are included in every export. Do
not export a day containing detailed events to anyone who is not authorised to
receive the recorded code changes and browser actions.

## Standard activity metadata

- X11 foreground application and window title.
- Firefox **active** tab URL and title. URL credentials, query strings and
  fragments are removed before transmission to the loopback service. Domains in
  `~/.config/intent-os/blocked-domains.yaml` are stored as `[blocked]`; detailed
  action context and target labels are removed for those domains.
- VS Code/Cursor workspace path, active local file path, language ID, edit/save
  signals.
- Shell command, working directory, exit code and duration. Commands matching
  likely secret terms and commands over 500 characters are stored as
  `<redacted>`.
- Optional workspace fallback path-change metadata. It ignores `.git`, common
  dependency/build folders, symlinks and files larger than 10 MB.

## Opt-in detailed capture

Detailed capture is disabled by default. It requires both the local Intent OS
consent switch and the VS Code/Cursor `intentOS.detailedCapture` setting for
editor changes. Firefox detailed actions require the local browser consent
switch. Status shows both settings and detailed-event counts.

- **Editor:** only approved workspaces can emit `vscode/document_change`
  events. An event records inserted or replacement text (up to 8 KB per event),
  pre-change range coordinates, and deleted-character counts. It never stores a
  full document snapshot or deleted text. `.env`, key files, SSH key names and
  paths containing secret/credential/password/token are excluded. Private keys,
  credential assignments and common token prefixes in inserted text are replaced
  with `[redacted]` before SQLite storage.
- **Firefox:** records trusted clicks, link activations, form submissions,
  checkbox/radio toggles and select changes on HTTP(S) pages. Normal-page events
  contain a bounded element label/role and sanitized destination URL where
  relevant. Private windows create no detailed events. On detected login,
  account, billing, checkout, password, or payment pages, only click/submit plus
  element tag/role are retained.

Intent OS never records browser form values, keyboard events, text-area or
contenteditable text, clipboard data, page HTML, DOM snapshots, network traffic,
pointer coordinates, editor selections, terminal output, or cloud telemetry.

Detailed-capture consent is stored at
`~/.config/intent-os/detailed-capture.json`. The editor exclusion list is
user-editable in that file. Run `intent-osctl purge-detailed` to delete all
stored `vscode/document_change` and `firefox/user_action` events while
preserving standard metadata.
