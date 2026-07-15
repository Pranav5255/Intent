# Demo-day fixture

Recording date: **2026-07-13** (local Ubuntu demo scenario).

`demo-day.json` contains 26 schema-versioned events for the infrastructure
story: 5 Linux focus events, 8 VS Code events, 7 Firefox events, and 5 shell
commands. It includes `iam.tf`, AWS IAM/Terraform research, repeated
AccessDenied investigation, and two failed `terraform apply` commands.

Replay it against a running Role A service:

```bash
python3 scripts/emit_fixture.py fixtures/demo-day.json
intent-osctl export-day --date 2026-07-13
```

For a real capture, keep all connectors enabled for a work session and export
the resulting day with the same command; do not overwrite this deterministic
fixture used by the demo and offline intent-engine tests.
