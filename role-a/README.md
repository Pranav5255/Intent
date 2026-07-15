# Intent OS — Role A

Local Ubuntu activity capture and restoration service. It captures standard
application, editor, browser, filesystem and shell metadata. Opt-in detailed
capture can record bounded editor insertions/replacements from approved
workspaces and semantic Firefox interactions without collecting browser form
values or document snapshots.

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
