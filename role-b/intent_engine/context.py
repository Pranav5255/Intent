"""Resume-safe markdown context export for one intent."""

from __future__ import annotations

from intent_engine.schemas import Intent


MAX_MARKDOWN_CHARS = 2048


def build_intent_context(intent: Intent) -> str:
    lines = [f"# {intent.label}", "", intent.summary.strip(), ""]

    lines.extend(_resume_section(intent))
    lines.extend(_insights_section(intent))
    lines.extend(_todos_section(intent))

    markdown = "\n".join(line for line in lines if line is not None).strip() + "\n"
    if len(markdown) <= MAX_MARKDOWN_CHARS:
        return markdown
    return markdown[: MAX_MARKDOWN_CHARS - 3].rstrip() + "...\n"


def _resume_section(intent: Intent) -> list[str]:
    lines = ["## Resume"]
    payload = intent.resume_payload
    if payload.files:
        for path in payload.files:
            lines.append(f"- File: {path}")
    if payload.urls:
        for url in payload.urls:
            lines.append(f"- URL: {url}")
    shell = payload.shell or {}
    cwd = shell.get("cwd")
    last_cmd = shell.get("last_cmd")
    if isinstance(cwd, str) and cwd:
        suffix = f" (last: {last_cmd})" if isinstance(last_cmd, str) and last_cmd else ""
        lines.append(f"- Shell: {cwd}{suffix}")
    if len(lines) == 1:
        lines.append("- No restore context stored.")
    lines.append("")
    return lines


def _insights_section(intent: Intent) -> list[str]:
    lines = ["## Insights"]
    editor = _editor_summary(intent)
    browser = _browser_summary(intent)
    shell = _shell_summary(intent)
    if editor:
        lines.append(f"- Editor: {editor}")
    if browser:
        lines.append(f"- Browser: {browser}")
    if shell:
        lines.append(f"- Shell: {shell}")
    if len(lines) == 1:
        lines.append("- No additional insights.")
    lines.append("")
    return lines


def _todos_section(intent: Intent) -> list[str]:
    todos = intent.todos
    if not todos:
        return []
    lines = ["## TODOs"]
    for todo in todos[:5]:
        lines.append(f"- {todo.marker} in {todo.path}")
    lines.append("")
    return lines


def _editor_summary(intent: Intent) -> str | None:
    files = [item.get("file") for item in intent.insights.editor if isinstance(item.get("file"), str)]
    if not files:
        for child in intent.children:
            files.extend(item.get("file") for item in child.insights.editor if isinstance(item.get("file"), str))
    unique = list(dict.fromkeys(files))
    if not unique:
        return None
    if len(unique) == 1:
        return f"heavy edits on {unique[0]}"
    return f"edits across {', '.join(unique[:3])}"


def _browser_summary(intent: Intent) -> str | None:
    domains = [item.get("domain") for item in intent.insights.browser if isinstance(item.get("domain"), str)]
    for child in intent.children:
        domains.extend(item.get("domain") for item in child.insights.browser if isinstance(item.get("domain"), str))
    unique = list(dict.fromkeys(domains))
    if not unique:
        return None
    return f"research on {', '.join(unique[:3])}"


def _shell_summary(intent: Intent) -> str | None:
    parts: list[str] = []
    for node in [intent, *intent.children]:
        for item in node.insights.shell:
            family = item.get("command_family")
            count = item.get("count")
            exit_code = item.get("exit_code")
            if isinstance(family, str) and isinstance(count, int) and count > 0:
                suffix = f" (exit {exit_code})" if isinstance(exit_code, int) else ""
                parts.append(f"{family} test failed{suffix}")
    unique = list(dict.fromkeys(parts))
    if not unique:
        return None
    return "; ".join(unique[:2])
