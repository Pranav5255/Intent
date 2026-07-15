# Intent OS — Role A

Local Ubuntu activity capture and restoration service. It captures standard
application, editor, browser, filesystem and shell metadata. Opt-in detailed
capture can record bounded editor insertions/replacements from approved
workspaces and semantic Firefox interactions without collecting browser form
values or document snapshots.

## Demo data and desktop flow

`fixtures/demo-day.json` is a 26-event, schema-versioned infrastructure
scenario (VS Code, Firefox, shell, and focus events) recorded for the locked
demo date **2026-07-13**. Replay it locally with `scripts/emit_fixture.py`,
then run the intent pipeline supplied by Role B.

After installation, the autostart entry starts local capture and calls
`intent-osctl notify-yesterday`. The command waits for the local event and
intent APIs, shows a native notification for the top inferred intent, and opens
the local app if its **Open** action is chosen. `intent-osctl tray` starts the
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
