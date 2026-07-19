# Demo-day fixture

Recording date: **2026-07-13** (local Ubuntu demo scenario).

`demo-day.json` contains 28 schema-versioned events for the **Building Login
Feature** story: Linux focus and idle events, VS Code work in `src/auth.tsx`,
Firefox research, and shell commands. It includes MDN Fetch API and Stack
Overflow JWT research, two failed `npm test -- auth` commands, and `npm run
dev` in `~/projects/taskflow-app`.

Replay it against a running Role A service:

```bash
python3 scripts/emit_fixture.py fixtures/demo-day.json
intent-osctl export-day --date 2026-07-13
```

For a real capture, keep all connectors enabled for a work session and export
the resulting day with the same command; do not overwrite this deterministic
fixture used by the demo and offline intent-engine tests.
