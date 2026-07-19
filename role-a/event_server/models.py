"""Canonical event and export models shared by every Role A collector."""

from __future__ import annotations

from typing import Any
from uuid import UUID

from pydantic import BaseModel, Field, root_validator, validator


EVENT_PAYLOAD_FIELDS: dict[tuple[str, str], tuple[str, ...]] = {
    ("linux", "app_focus"): ("app", "title"),
    ("linux", "idle_start"): (),
    ("linux", "idle_end"): (),
    ("firefox", "tab_change"): ("url", "title"),
    ("firefox", "tab_close"): ("url",),
    ("firefox", "user_action"): ("url", "tab_id", "window_id", "action", "target", "sensitive_page"),
    ("vscode", "workspace_open"): ("folder",),
    ("vscode", "file_open"): ("path",),
    ("vscode", "file_edit"): ("path",),
    ("vscode", "file_save"): ("path",),
    ("vscode", "document_change"): ("path", "workspace", "changes"),
    ("filesystem", "file_modify"): ("path", "workspace"),
    ("filesystem", "workspace_seen"): ("workspace",),
    ("filesystem", "file_content"): ("path", "workspace", "kind", "mime", "size_bytes", "sha256", "excerpt"),
    ("shell", "command"): ("cmd", "cwd", "exit_code"),
}

MAX_DOCUMENT_CHANGES = 25
MAX_DOCUMENT_TEXT_BYTES = 8 * 1024
CHANGE_KINDS = {"insert", "delete", "replace"}
USER_ACTIONS = {"click", "link_activation", "form_submit", "toggle", "select_change", "like", "reply", "repost", "share", "follow", "unfollow"}


def _bounded_string(value: object, field: str, maximum: int) -> None:
    if not isinstance(value, str) or not value:
        raise ValueError(f"payload {field} must be a non-empty string")
    if len(value) > maximum:
        raise ValueError(f"payload {field} must be at most {maximum} characters")


def _position(value: object, field: str) -> None:
    if not isinstance(value, dict) or set(value) != {"line", "character"}:
        raise ValueError(f"payload {field} must contain line and character")
    if not all(isinstance(value[item], int) and value[item] >= 0 for item in value):
        raise ValueError(f"payload {field} coordinates must be non-negative integers")


def _validate_document_change(payload: dict[str, Any]) -> None:
    allowed_payload_fields = {"path", "workspace", "language", "changes"}
    if set(payload) - allowed_payload_fields:
        raise ValueError("document_change payload contains unsupported fields")
    _bounded_string(payload["path"], "path", 4096)
    _bounded_string(payload["workspace"], "workspace", 4096)
    if "language" in payload:
        _bounded_string(payload["language"], "language", 64)
    changes = payload["changes"]
    if not isinstance(changes, list) or not 1 <= len(changes) <= MAX_DOCUMENT_CHANGES:
        raise ValueError(f"payload changes must contain 1 to {MAX_DOCUMENT_CHANGES} items")
    total_bytes = 0
    for index, change in enumerate(changes):
        if not isinstance(change, dict):
            raise ValueError(f"payload changes[{index}] must be an object")
        required = {"kind", "range", "removed_characters"}
        allowed = required | {"text", "text_length", "redacted"}
        if not required.issubset(change) or set(change) - allowed:
            raise ValueError(f"payload changes[{index}] has invalid fields")
        if change["kind"] not in CHANGE_KINDS:
            raise ValueError(f"payload changes[{index}].kind is invalid")
        range_value = change["range"]
        if not isinstance(range_value, dict) or set(range_value) != {"start", "end"}:
            raise ValueError(f"payload changes[{index}].range must contain start and end")
        _position(range_value["start"], f"changes[{index}].range.start")
        _position(range_value["end"], f"changes[{index}].range.end")
        if not isinstance(change["removed_characters"], int) or change["removed_characters"] < 0:
            raise ValueError(f"payload changes[{index}].removed_characters must be a non-negative integer")
        text = change.get("text")
        if change["kind"] in {"insert", "replace"} and not isinstance(text, str):
            raise ValueError(f"payload changes[{index}].text is required for insert and replace")
        if text is not None and not isinstance(text, str):
            raise ValueError(f"payload changes[{index}].text must be a string")
        if isinstance(text, str):
            total_bytes += len(text.encode("utf-8"))
        if "text_length" in change and (
            not isinstance(change["text_length"], int) or change["text_length"] < 0
        ):
            raise ValueError(f"payload changes[{index}].text_length must be a non-negative integer")
        if "redacted" in change and not isinstance(change["redacted"], bool):
            raise ValueError(f"payload changes[{index}].redacted must be a boolean")
    if total_bytes > MAX_DOCUMENT_TEXT_BYTES:
        raise ValueError(f"payload changes text must be at most {MAX_DOCUMENT_TEXT_BYTES} bytes")


def _validate_user_action(payload: dict[str, Any]) -> None:
    allowed_payload_fields = {"url", "tab_id", "window_id", "action", "target", "sensitive_page", "context", "blocked"}
    if set(payload) - allowed_payload_fields:
        raise ValueError("user_action payload contains unsupported fields")
    _bounded_string(payload["url"], "url", 4096)
    if not isinstance(payload["tab_id"], int) or payload["tab_id"] < 0:
        raise ValueError("payload tab_id must be a non-negative integer")
    if not isinstance(payload["window_id"], int) or payload["window_id"] < 0:
        raise ValueError("payload window_id must be a non-negative integer")
    if payload["action"] not in USER_ACTIONS:
        raise ValueError("payload action is invalid")
    if not isinstance(payload["sensitive_page"], bool):
        raise ValueError("payload sensitive_page must be a boolean")
    if "blocked" in payload and not isinstance(payload["blocked"], bool):
        raise ValueError("payload blocked must be a boolean")
    target = payload["target"]
    if not isinstance(target, dict) or not {"tag", "role"}.issubset(target):
        raise ValueError("payload target must contain tag and role")
    allowed = {"tag", "role", "label", "input_type", "href", "checked"}
    if set(target) - allowed:
        raise ValueError("payload target contains unsupported fields")
    _bounded_string(target["tag"], "target.tag", 64)
    if not isinstance(target["role"], str) or len(target["role"]) > 64:
        raise ValueError("payload target.role must be a string of at most 64 characters")
    if payload["sensitive_page"]:
        if payload["action"] not in {"click", "form_submit"}:
            raise ValueError("sensitive-page actions must be click or form_submit")
        if set(target) != {"tag", "role"}:
            raise ValueError("sensitive-page targets may contain only tag and role")
        return
    for field, maximum in (("label", 160), ("input_type", 64), ("href", 4096)):
        if field in target and (not isinstance(target[field], str) or len(target[field]) > maximum):
            raise ValueError(f"payload target.{field} must be a string of at most {maximum} characters")
    if "checked" in target and not isinstance(target["checked"], bool):
        raise ValueError("payload target.checked must be a boolean")
    if "context" in payload:
        context = payload["context"]
        if not isinstance(context, dict) or set(context) - {"kind", "author", "text_excerpt"}:
            raise ValueError("payload context has invalid fields")
        _bounded_string(context.get("kind"), "context.kind", 32)
        if "author" in context:
            _bounded_string(context["author"], "context.author", 160)
        _bounded_string(context.get("text_excerpt"), "context.text_excerpt", 1000)


def _validate_file_content(payload: dict[str, Any]) -> None:
    allowed = {"path", "workspace", "kind", "mime", "size_bytes", "sha256", "excerpt"}
    if set(payload) - allowed:
        raise ValueError("file_content payload contains unsupported fields")
    for field, maximum in (("path", 4096), ("workspace", 4096), ("kind", 16), ("mime", 128), ("sha256", 64), ("excerpt", 4000)):
        _bounded_string(payload[field], field, maximum)
    if payload["kind"] not in {"text", "pdf", "image"}:
        raise ValueError("file_content payload kind is invalid")
    if not isinstance(payload["size_bytes"], int) or payload["size_bytes"] < 0:
        raise ValueError("file_content payload size_bytes must be a non-negative integer")


class EventIn(BaseModel):
    """An immutable observed activity event."""

    id: UUID
    # Keep exports forward-compatible without introducing user or device identity.
    schema_version: int = Field(default=1, ge=1, le=99)
    ts: int = Field(ge=0, description="UTC Unix seconds")
    source: str = Field(min_length=1, max_length=32)
    type: str = Field(min_length=1, max_length=64)
    payload: dict[str, Any] = Field(default_factory=dict)

    @validator("source", "type")
    def lowercase_identifiers(cls, value: str) -> str:
        return value.strip().lower()

    @root_validator(skip_on_failure=True)
    def validate_event_kind(cls, values: dict[str, Any]) -> dict[str, Any]:
        source = values["source"]
        event_type = values["type"]
        payload = values["payload"]
        required = EVENT_PAYLOAD_FIELDS.get((source, event_type))
        if required is None:
            supported = ", ".join(
                f"{item_source}/{item_type}" for item_source, item_type in EVENT_PAYLOAD_FIELDS
            )
            raise ValueError(f"unsupported event kind {source}/{event_type}; supported: {supported}")

        missing = [field for field in required if field not in payload]
        if missing:
            raise ValueError("payload missing required fields: " + ", ".join(missing))
        for field in required:
            if payload[field] is None:
                raise ValueError(f"payload field {field} cannot be null")
        if (source, event_type) == ("vscode", "document_change"):
            _validate_document_change(payload)
        if (source, event_type) == ("firefox", "user_action"):
            _validate_user_action(payload)
        if (source, event_type) == ("filesystem", "file_content"):
            _validate_file_content(payload)
        return values

    def as_record(self) -> dict[str, Any]:
        return {
            "id": str(self.id),
            "schema_version": self.schema_version,
            "ts": self.ts,
            "source": self.source,
            "type": self.type,
            "payload": self.payload,
        }


class EventOut(EventIn):
    ingested_at: int


class DayExport(BaseModel):
    version: int = 1
    date: str
    exported_at: int
    events: list[EventOut]


class IngestResult(BaseModel):
    ok: bool = True
    inserted: bool
    event: EventOut


class CapturePause(BaseModel):
    paused: bool
