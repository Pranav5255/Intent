"""Deterministic conversion of Role A activity events into pipeline events."""

from __future__ import annotations

import hashlib
import json
import re
from pathlib import PurePath
from typing import Any

from intent_engine.schemas import ContextEvidence, EventEntities, EventSignals, NormalizedEvent, PipelineWarning, RawEvent


_COMMAND_FAMILIES = {name: name for name in ("terraform", "git", "pytest", "python", "npm", "pip", "docker", "make")}
_CODE_EXTENSIONS = {".py", ".js", ".ts", ".go", ".java", ".cpp", ".c", ".tf", ".yaml", ".json", ".sh"}
_IMAGE_EXTENSIONS = {".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg", ".bmp"}
_TODO_MARKERS = ("TODO", "FIXME", "XXX")


def derive_command_family(command: str) -> str | None:
    """Return the supported family for a shell command's first token."""

    parts = command.split()
    if not parts:
        return None
    return _COMMAND_FAMILIES.get(parts[0].lower())


def derive_file_kind(file_path: str) -> str | None:
    """Classify a file path by extension without reading the filesystem."""

    if not file_path:
        return None
    extension = PurePath(file_path.replace("\\", "/")).suffix.lower()
    if extension in _CODE_EXTENSIONS:
        return "code"
    if extension == ".pdf":
        return "pdf"
    if extension in _IMAGE_EXTENSIONS:
        return "image"
    return "other"


def extract_domain_from_url(url: str) -> str | None:
    """Return an HTTP(S) URL's authority without retaining the full URL."""

    if not url.startswith(("http://", "https://")):
        return None
    authority = url.split("://", 1)[1].split("/", 1)[0].split("?", 1)[0]
    return authority or None


def generate_normalized_text(
    event_type: str,
    source: str,
    family: str,
    entities: EventEntities,
    signals: EventSignals,
) -> str:
    """Build a concise activity description from extracted metadata only."""

    if family == "editor":
        if entities.file_name:
            return f"Edited {entities.file_name}"
        if signals.typed_chars:
            return f"Typed {signals.typed_chars} chars"
        return "Edited file"
    if family == "browser":
        return f"Viewed {entities.domain}" if entities.domain else "Viewed browser page"
    if family == "command":
        text = f"Ran {entities.command_family}" if entities.command_family else "Ran command"
        if entities.exit_code not in (None, 0):
            text += f" (exit code {entities.exit_code})"
        return text
    if family == "focus":
        return f"Focused on {entities.title}" if entities.title else "Focused on application"
    if family == "idle":
        return "Idle period"
    if family == "file_change":
        return f"File modified: {entities.file_path}" if entities.file_path else "File modified"
    return f"Observed {source}/{event_type}"


def normalize_event(raw: RawEvent, ordinal: int) -> tuple[NormalizedEvent | None, PipelineWarning | None]:
    """Normalize one event, returning a typed warning instead of raising on failure."""

    event_id = getattr(raw, "id", None)
    try:
        source = raw.source.strip().lower()
        event_type = raw.type.strip().lower()
        payload = raw.payload.model_dump()
        family = _derive_family(source, event_type)
        evidence = extract_evidence(payload)
        entities = _extract_entities(payload, family, evidence)
        signals = _extract_signals(payload, event_type)
        text = generate_normalized_text(event_type, source, family, entities, signals)
        normalized = NormalizedEvent(
            id=raw.id,
            ts=raw.ts,
            ordinal=ordinal,
            source=source,
            family=family,
            category=event_type,
            text=text,
            entities=entities,
            signals=signals,
            evidence=evidence,
            raw=raw.model_dump(mode="json"),
        )
        return normalized, None
    except Exception as exc:
        return None, PipelineWarning(
            level="warning",
            message=f"Normalization failed: {exc.__class__.__name__}",
            event_id=event_id if isinstance(event_id, str) else None,
        )


def normalize_events(raw_events: list[RawEvent]) -> tuple[list[NormalizedEvent], list[PipelineWarning]]:
    """Sort, deduplicate, and normalize a batch of Role A events."""

    normalized_events: list[NormalizedEvent] = []
    warnings: list[PipelineWarning] = []
    seen_ids: set[str] = set()

    for original_ordinal, raw in sorted(enumerate(raw_events), key=lambda item: (item[1].ts, item[0])):
        if raw.id in seen_ids:
            continue
        seen_ids.add(raw.id)
        normalized, warning = normalize_event(raw, original_ordinal)
        if normalized is not None:
            normalized_events.append(normalized)
        if warning is not None:
            warnings.append(warning)

    return normalized_events, warnings


def compute_source_hash(normalized_events: list[NormalizedEvent]) -> str:
    """Return a compact, deterministic SHA-256 identifier for normalized input."""

    canonical_json = json.dumps(
        [event.model_dump(mode="json") for event in normalized_events],
        sort_keys=True,
        separators=(",", ":"),
    )
    return hashlib.sha256(canonical_json.encode("utf-8")).hexdigest()[:16]


def intelligence_text(event: NormalizedEvent, ordinal: int | None = None) -> str:
    """Render an event's complete consent-approved context for Role B reasoning."""

    prefix = f"{ordinal}. " if ordinal is not None else ""
    lines = [f"{prefix}[{event.source}/{event.category}] {event.text}"]
    lines.extend(f"  {item.field}: {item.value}" for item in event.evidence)
    return "\n".join(lines)


def _derive_family(source: str, event_type: str) -> str:
    if source == "vscode":
        return "editor"
    if source in {"firefox", "chrome"}:
        return "browser"
    if source == "linux":
        return "idle" if event_type.startswith("idle_") else "focus"
    if source == "shell":
        return "command"
    if source == "filesystem":
        return "file_change"
    return "other"


def _extract_entities(payload: dict[str, Any], family: str, evidence: list[ContextEvidence]) -> EventEntities:
    project_paths = _project_paths(payload)
    file_path = _string(payload.get("path"))
    command = _string(payload.get("cmd"))
    url = _string(payload.get("url"))
    title = _string(payload.get("title"))
    cwd = _string(payload.get("cwd"))
    exit_code = payload.get("exit_code")

    return EventEntities(
        project_paths=project_paths,
        file_path=file_path,
        file_name=_file_name(file_path),
        file_kind=derive_file_kind(file_path) if file_path else None,
        domain=extract_domain_from_url(url) if url else None,
        title=title,
        command=command,
        command_family=derive_command_family(command) if command else None,
        cwd=cwd,
        exit_code=exit_code if isinstance(exit_code, int) and not isinstance(exit_code, bool) else None,
        context_terms=_context_terms(evidence),
    )


def _extract_signals(payload: dict[str, Any], event_type: str) -> EventSignals:
    typed_chars = payload.get("typed_chars")
    if not isinstance(typed_chars, int) or isinstance(typed_chars, bool) or typed_chars < 0:
        typed_chars = _document_change_characters(payload)
    return EventSignals(
        typed_chars=typed_chars,
        save="save" in event_type,
        todo_added=bool(payload.get("todo_added")) or _document_change_has_todo(payload),
    )


def _project_paths(payload: dict[str, Any]) -> list[str]:
    paths: list[str] = []
    for key in ("cwd", "workspace", "folder"):
        value = _string(payload.get(key))
        if value and value not in paths:
            paths.append(value)
    return paths


def _document_change_characters(payload: dict[str, Any]) -> int:
    total = 0
    changes = payload.get("changes")
    if not isinstance(changes, list):
        return total
    for change in changes:
        if not isinstance(change, dict):
            continue
        length = change.get("text_length")
        if isinstance(length, int) and not isinstance(length, bool) and length >= 0:
            total += length
        elif not change.get("redacted") and isinstance(change.get("text"), str):
            total += len(change["text"])
    return total


def _document_change_has_todo(payload: dict[str, Any]) -> bool:
    changes = payload.get("changes")
    if not isinstance(changes, list):
        return False
    for change in changes:
        text = change.get("text") if isinstance(change, dict) and not change.get("redacted") else None
        if isinstance(text, str) and any(marker in text.upper() for marker in _TODO_MARKERS):
            return True
    return False


def extract_evidence(payload: dict[str, Any]) -> list[ContextEvidence]:
    """Flatten every Role A payload value into deterministic LLM-ready evidence.

    Role A is the consent and redaction boundary.  Role B must therefore retain
    every value Role A exports rather than silently selecting a small subset of
    fields during normalization.  List indexes and nested field names preserve
    the source of editor changes, browser context, and file excerpts.
    """

    evidence: list[ContextEvidence] = []

    def visit(field: str, value: object) -> None:
        if isinstance(value, dict):
            for key in sorted(value):
                visit(f"{field}.{key}" if field else str(key), value[key])
            return
        if isinstance(value, list):
            for index, item in enumerate(value):
                visit(f"{field}[{index}]", item)
            return
        if value is None:
            return
        if isinstance(value, bool):
            rendered = "true" if value else "false"
        elif isinstance(value, (str, int, float)):
            rendered = _normalise_evidence_value(str(value))
        else:
            rendered = _normalise_evidence_value(json.dumps(value, sort_keys=True, ensure_ascii=False))
        if rendered:
            evidence.append(ContextEvidence(field=field, value=rendered))

    visit("", payload)
    return evidence


def _normalise_evidence_value(value: str) -> str:
    """Keep captured content intact apart from whitespace-only normalization."""

    return re.sub(r"\s+", " ", value).strip()


def _context_terms(evidence: list[ContextEvidence]) -> list[str]:
    """Derive stable topic tokens from every consent-approved payload value."""

    ignored = {
        "the", "and", "for", "with", "from", "this", "that", "into", "true", "false",
        "http", "https", "www", "com", "org", "net", "text", "insert", "delete", "replace",
    }
    terms: list[str] = []
    for item in evidence:
        for token in re.findall(r"[A-Za-z][A-Za-z0-9_-]{2,}", item.value.lower()):
            if token not in ignored and token not in terms:
                terms.append(token)
    return terms[:64]


def _string(value: object) -> str | None:
    return value if isinstance(value, str) and value else None


def _file_name(file_path: str | None) -> str | None:
    if not file_path:
        return None
    return file_path.replace("\\", "/").rsplit("/", 1)[-1] or None
