# Intent — Role A

Local Ubuntu activity capture and restoration service. It captures standard
application, editor, browser, filesystem and shell metadata. Opt-in detailed
capture can record bounded editor insertions/replacements from approved
workspaces and semantic Firefox interactions without collecting browser form
values or document snapshots.

## Demo data and desktop flow

`fixtures/demo-day.json` is a 28-event, schema-versioned **Building Login
Feature** scenario (VS Code, Firefox, shell, focus, and idle events) recorded
for the locked demo date **2026-07-13**. It follows `src/auth.tsx`, failed
`npm test` runs, and MDN/Stack Overflow JWT research. Replay it locally with
`scripts/emit_fixture.py`, then run the intent pipeline supplied by Role B.

After installation, the autostart entry starts local capture and calls
`intent-osctl notify-yesterday`. The command waits for the local event and
intent APIs, then shows `Continue {project}?` for the top inferred intent. Its
**Preview** action starts only the explicitly configured
`INTENT_OS_PREVIEW_COMMAND`, passing a Role C preview URL; it never restores
applications. Role C must show the Role B preview and wait for a separate user
confirmation before it calls `POST /v1/restore`. `intent-osctl tray` starts the
GNOME/Ayatana indicator with Yesterday's intents, Pause capture, Open app, and
Quit actions. The indicator degrades harmlessly when an AppIndicator host is
not installed.

`POST /v1/restore` accepts `mode: "resume"` (full restore) or
`mode: "continue"` (skips files visible in open windows and opens a terminal
tab). All event responses and exports include `schema_version` (default `1`).

## Development

```bash
python3 -m venv .venv
.venv/bin/pip install -r requirements.txt
.venv/bin/uvicorn event_server.main:app --host 127.0.0.1 --port 9477
```

Run the complete test suite with:

```bash
make test
```

## Local privacy and health

Copy `config/blocked-domains.yaml.example` to
`~/.config/intent-os/blocked-domains.yaml` to exclude a browser domain and all
of its subdomains from capture. Blocked events remain visible only as
`[blocked]` audit records. `GET /v1/status` reports recent connector health,
activity timing, and service availability; source events become stale after 30
minutes by default (`INTENT_OS_SOURCE_STALE_AFTER_SECONDS` overrides it).
