"""Deterministic daily digest built from persisted intents."""

from __future__ import annotations

from intent_engine.schemas import Intent


HEADLINE_OVERRIDES = {
    "project:taskflow-app": "Building Login Feature",
}


def build_digest(intents: list[Intent], date: str) -> dict:
    roots = [intent for intent in intents if intent.depth == 0]
    if not roots:
        return {
            "date": date,
            "headline": "No recorded work",
            "summary": "No intents were stored for this date.",
            "top_intent_ids": [],
            "intent_count": 0,
            "total_duration_seconds": 0,
        }

    primary = _primary_root(roots)
    headline = _headline(primary)
    summary = _summary(primary)
    total_duration = sum(intent.stats.duration_seconds for intent in roots)
    return {
        "date": date,
        "headline": headline,
        "summary": summary,
        "top_intent_ids": [intent.id for intent in roots[:3]],
        "intent_count": len(roots),
        "total_duration_seconds": total_duration,
    }


def _primary_root(roots: list[Intent]) -> Intent:
    tagged = [root for root in roots if any(tag.startswith("project:") for tag in root.tags)]
    candidates = tagged or roots
    return max(candidates, key=lambda root: (len(root.children), root.stats.duration_seconds))


def _headline(root: Intent) -> str:
    for tag in root.tags:
        if tag in HEADLINE_OVERRIDES:
            return HEADLINE_OVERRIDES[tag]
    return root.label


def _summary(root: Intent) -> str:
    parts: list[str] = []
    child_labels = [child.label for child in root.children if child.label]
    if child_labels:
        parts.append(f"You worked on {', '.join(child_labels[:2]).lower()}")

    editor_files = _editor_files(root)
    if editor_files:
        parts.append(f"edited {editor_files[0]}")

    domains = _browser_domains(root)
    if domains:
        parts.append(f"researched {domains[0]}")

    failed = _failed_commands(root)
    if failed:
        parts.append(f"hit failing {failed[0]} commands")

    if not parts:
        return root.summary

    sentence = ", ".join(parts[:3]).rstrip(". ")
    if not sentence.endswith("."):
        sentence += "."
    return sentence[0].upper() + sentence[1:]


def _editor_files(root: Intent) -> list[str]:
    files: list[str] = []
    for node in [root, *root.children]:
        for item in node.insights.editor:
            file_name = item.get("file")
            if isinstance(file_name, str) and file_name not in files:
                files.append(file_name)
    return files


def _browser_domains(root: Intent) -> list[str]:
    domains: list[str] = []
    for node in [root, *root.children]:
        for item in node.insights.browser:
            domain = item.get("domain")
            if isinstance(domain, str) and domain not in domains:
                domains.append(domain)
    return domains


def _failed_commands(root: Intent) -> list[str]:
    families: list[str] = []
    for node in [root, *root.children]:
        for item in node.insights.shell:
            family = item.get("command_family")
            count = item.get("count")
            if isinstance(family, str) and isinstance(count, int) and count > 0 and family not in families:
                families.append(family)
    return families
