from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parents[1]))

from intent_engine.normalize import (
    compute_source_hash,
    derive_command_family,
    derive_file_kind,
    extract_domain_from_url,
    normalize_event,
    normalize_events,
)
from intent_engine.schemas import RawEvent


def raw_event(event_id: str, ts: int, source: str, event_type: str, payload: dict) -> RawEvent:
    return RawEvent(id=event_id, ts=ts, source=source, type=event_type, payload=payload)


def test_derivation_helpers() -> None:
    assert derive_command_family("terraform apply") == "terraform"
    assert derive_command_family("  GIT status") == "git"
    assert derive_command_family("") is None
    assert derive_command_family("unknown command") is None
    assert derive_file_kind("main.PY") == "code"
    assert derive_file_kind("report.pdf") == "pdf"
    assert derive_file_kind("image.WEBP") == "image"
    assert derive_file_kind("notes.txt") == "other"
    assert derive_file_kind("") is None
    assert extract_domain_from_url("https://docs.aws.amazon.com/iam/") == "docs.aws.amazon.com"
    assert extract_domain_from_url("[REDACTED]") is None


def test_normalizes_editor_and_command_without_document_text() -> None:
    editor = raw_event(
        "editor", 2, "vscode", "document_change",
        {"path": "/work/iam.tf", "workspace": "/work", "changes": [{"text": "# TODO update role"}]},
    )
    command = raw_event("command", 3, "shell", "command", {"cmd": "terraform apply", "cwd": "/work", "exit_code": 1})

    normalized_editor, editor_warning = normalize_event(editor, 0)
    normalized_command, command_warning = normalize_event(command, 1)

    assert editor_warning is None and normalized_editor is not None
    assert normalized_editor.family == "editor"
    assert normalized_editor.category == "document_change"
    assert normalized_editor.entities.file_kind == "code"
    assert normalized_editor.signals.typed_chars == len("# TODO update role")
    assert normalized_editor.signals.todo_added is True
    assert "TODO update role" not in normalized_editor.text
    assert command_warning is None and normalized_command is not None
    assert normalized_command.text == "Ran terraform (exit code 1)"


def test_retains_all_role_a_payload_values_as_intelligence_evidence() -> None:
    browser = raw_event(
        "browser", 4, "firefox", "user_action",
        {
            "url": "https://docs.example.com/guide",
            "action": "click",
            "target": {"tag": "button", "role": "button", "label": "Deploy"},
            "context": {"kind": "article", "author": "Ada", "text_excerpt": "Use a canary deployment."},
        },
    )
    file_content = raw_event(
        "file", 5, "filesystem", "file_content",
        {"path": "/work/plan.md", "kind": "text", "mime": "text/markdown", "excerpt": "Deploy after approval."},
    )

    normalized_browser, browser_warning = normalize_event(browser, 0)
    normalized_file, file_warning = normalize_event(file_content, 1)

    assert browser_warning is None and normalized_browser is not None
    assert file_warning is None and normalized_file is not None
    browser_evidence = {item.field: item.value for item in normalized_browser.evidence}
    file_evidence = {item.field: item.value for item in normalized_file.evidence}
    assert browser_evidence["target.label"] == "Deploy"
    assert browser_evidence["context.author"] == "Ada"
    assert browser_evidence["context.text_excerpt"] == "Use a canary deployment."
    assert file_evidence["excerpt"] == "Deploy after approval."


def test_batch_sorting_deduplication_and_hashing() -> None:
    late = raw_event("late", 20, "linux", "app_focus", {"title": "Code"})
    early = raw_event("early", 10, "filesystem", "file_modify", {"path": "/work/image.png", "workspace": "/work"})
    duplicate = raw_event("early", 30, "chrome", "tab_change", {"url": "https://example.com/"})

    normalized, warnings = normalize_events([late, early, duplicate])

    assert warnings == []
    assert [event.id for event in normalized] == ["early", "late"]
    assert normalized[0].family == "file_change"
    assert compute_source_hash(normalized) == compute_source_hash(normalized)
    assert len(compute_source_hash(normalized)) == 16
