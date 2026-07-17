from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parents[1]))

from intent_engine.resume import build_resume_payload, merge_resume_payloads
from intent_engine.schemas import EventEntities, NormalizedEvent, ResumePayload


def event(
    event_id: str,
    ts: int,
    *,
    family: str,
    ordinal: int = 0,
    file_path: str | None = None,
    url: str | None = None,
    cwd: str | None = None,
    command: str | None = None,
) -> NormalizedEvent:
    raw = {"payload": {"url": url}} if url is not None else {}
    return NormalizedEvent(
        id=event_id,
        ts=ts,
        ordinal=ordinal,
        source="test",
        family=family,
        category="event",
        text="Safe text",
        entities=EventEntities(file_path=file_path, cwd=cwd, command=command),
        raw=raw,
    )


def test_build_payload_orders_files_urls_and_shell_by_recency() -> None:
    cluster = [
        event("old-file", 1, family="editor", file_path="/work/old.py"),
        event("new-file", 10, family="editor", file_path="/work/new.py"),
        event("duplicate-file", 11, family="editor", file_path="/work/old.py"),
        event("old-url", 2, family="browser", url="https://docs.example.com/old"),
        event("new-same-domain", 12, family="browser", url="https://docs.example.com/new"),
        event("other-url", 13, family="browser", url="http://example.org/page"),
        event("shell", 20, family="command", cwd="/work", command="terraform apply"),
    ]

    payload = build_resume_payload(cluster)

    assert payload.files == ["/work/old.py", "/work/new.py"]
    assert payload.urls == ["http://example.org/page", "https://docs.example.com/new"]
    assert payload.shell == {"cwd": "/work", "last_cmd": "terraform apply"}


def test_build_payload_enforces_caps_and_filters_invalid_urls() -> None:
    files = [event(f"file-{index}", index, family="editor", file_path=f"/work/{index}.py") for index in range(6)]
    urls = [
        event("redacted", 20, family="browser", url="[REDACTED]"),
        event("file-url", 21, family="browser", url="file:///work/report.pdf"),
        event("internal", 22, family="browser", url="chrome://settings"),
        event("malformed", 23, family="browser", url="not a url"),
    ] + [event(f"url-{index}", 30 + index, family="browser", url=f"https://{index}.example.com/") for index in range(9)]

    payload = build_resume_payload(files + urls)

    assert len(payload.files) == 5
    assert payload.files[0] == "/work/5.py"
    assert len(payload.urls) == 8
    assert all(url.startswith(("http://", "https://")) for url in payload.urls)


def test_merge_payloads_treats_final_child_as_most_recent() -> None:
    older = ResumePayload(
        files=["/work/older.py", "/work/shared.py"],
        urls=["https://older.example/", "https://shared.example/old"],
        shell={"cwd": "/older", "last_cmd": "git status"},
    )
    newer = ResumePayload(
        files=["/work/newer.py", "/work/shared.py"],
        urls=["https://newer.example/", "https://shared.example/new"],
        shell={"cwd": "/newer"},
    )

    payload = merge_resume_payloads([older, newer])

    assert payload.files == ["/work/newer.py", "/work/shared.py", "/work/older.py"]
    assert payload.urls == ["https://newer.example/", "https://shared.example/new", "https://older.example/"]
    assert payload.shell == {"cwd": "/newer", "last_cmd": "git status"}


def test_empty_payloads_are_valid() -> None:
    assert build_resume_payload([]).model_dump() == {"files": [], "urls": [], "shell": {}}
    assert merge_resume_payloads([]).model_dump() == {"files": [], "urls": [], "shell": {}}
