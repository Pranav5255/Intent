"""Safe, best-effort local restoration for the original Ubuntu machine."""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path
from urllib.parse import urlparse

from pydantic import BaseModel, Field, validator


class ShellState(BaseModel):
    cwd: str | None = None
    last_cmd: str | None = Field(default=None, max_length=500)


class ResumePayload(BaseModel):
    mode: str = "resume"
    files: list[str] = Field(default_factory=list)
    urls: list[str] = Field(default_factory=list)
    shell: ShellState = Field(default_factory=ShellState)

    @validator("files")
    def unique_files(cls, value: list[str]) -> list[str]:
        value = list(dict.fromkeys(value))
        if len(value) > 5:
            raise ValueError("at most five files may be restored")
        return value

    @validator("mode")
    def valid_mode(cls, value: str) -> str:
        if value not in {"resume", "continue"}:
            raise ValueError("mode must be resume or continue")
        return value

    @validator("urls")
    def valid_urls(cls, value: list[str]) -> list[str]:
        result = list(dict.fromkeys(value))
        if len(result) > 8:
            raise ValueError("at most eight URLs may be restored")
        for url in result:
            parsed = urlparse(url)
            if parsed.scheme not in {"http", "https"} or not parsed.netloc:
                raise ValueError("urls must use http or https")
        return result


class RestoreResult(BaseModel):
    ok: bool
    restored: dict[str, int | bool]
    failed: list[str]


def _launch(arguments: list[str]) -> None:
    subprocess.Popen(arguments, start_new_session=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def _already_open_files(files: list[str]) -> set[str]:
    """Best-effort VS Code window inspection used only by Continue.

    wmctrl exposes window titles, not editor tabs, so this intentionally errs on
    the side of reopening a file rather than claiming it was restored.
    """
    wmctrl = shutil.which("wmctrl")
    if not wmctrl:
        return set()
    try:
        output = subprocess.run([wmctrl, "-l"], check=False, capture_output=True, text=True, timeout=1).stdout
    except (OSError, subprocess.SubprocessError):
        return set()
    return {item for item in files if Path(item).name in output}


def restore(payload: ResumePayload) -> RestoreResult:
    """Launch files, URLs and terminal state without using a shell interpreter."""
    restored: dict[str, int | bool] = {"files": 0, "urls": 0, "shell": False}
    failed: list[str] = []

    files = [Path(item).expanduser() for item in payload.files]
    existing_files = [str(item.resolve()) for item in files if item.is_file()]
    missing_files = [str(item) for item in files if not item.is_file()]
    if missing_files:
        failed.append(f"files not found: {', '.join(missing_files)}")
    if payload.mode == "continue":
        open_files = _already_open_files(existing_files)
        existing_files = [item for item in existing_files if item not in open_files]
    if existing_files:
        code = shutil.which("code")
        if code:
            try:
                _launch([code, "--reuse-window", *existing_files])
                restored["files"] = len(existing_files)
            except OSError as exc:
                failed.append(f"VS Code launch failed: {exc}")
        else:
            failed.append("VS Code CLI (code) is unavailable")

    if payload.urls:
        firefox = shutil.which("firefox")
        if firefox:
            try:
                arguments = [firefox, "--new-window", payload.urls[0]]
                for url in payload.urls[1:]:
                    arguments.extend(["--new-tab", url])
                _launch(arguments)
                restored["urls"] = len(payload.urls)
            except OSError as exc:
                failed.append(f"Firefox launch failed: {exc}")
        else:
            failed.append("Firefox is unavailable")

    if payload.shell.cwd:
        cwd = Path(payload.shell.cwd).expanduser()
        terminal = shutil.which("gnome-terminal")
        if not cwd.is_dir():
            failed.append(f"terminal working directory not found: {cwd}")
        elif not terminal:
            failed.append("GNOME Terminal is unavailable")
        else:
            try:
                # Do not pre-fill or execute last_cmd. It may contain sensitive or destructive input.
                arguments = [terminal]
                if payload.mode == "continue":
                    arguments.append("--tab")
                arguments.append(f"--working-directory={cwd.resolve()}")
                _launch(arguments)
                restored["shell"] = True
            except OSError as exc:
                failed.append(f"terminal launch failed: {exc}")

    return RestoreResult(ok=not failed, restored=restored, failed=failed)
