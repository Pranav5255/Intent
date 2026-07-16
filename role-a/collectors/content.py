"""Bounded local text extraction for explicitly observed file changes.

This module never walks the disk. It only examines a path after the filesystem
collector observed a create/modify event, and it intentionally ignores virtual
filesystems and credential-like paths.
"""

from __future__ import annotations

import hashlib
import mimetypes
import shutil
import subprocess
from pathlib import Path


MAX_FILE_BYTES = 25 * 1024 * 1024
MAX_EXCERPT_CHARS = 4_000
TEXT_SUFFIXES = {".txt", ".md", ".rst", ".csv", ".json", ".yaml", ".yml", ".toml", ".ini", ".py", ".js", ".ts", ".html", ".css", ".sh", ".xml"}
IMAGE_SUFFIXES = {".png", ".jpg", ".jpeg", ".webp", ".tiff", ".bmp"}
SENSITIVE_NAMES = {".env", "shadow", "gshadow", "passwd", "id_rsa", "id_ed25519", "known_hosts"}
SENSITIVE_PARTS = {".ssh", ".gnupg", ".pki", ".mozilla", ".config/google-chrome", ".config/chromium"}
VIRTUAL_ROOTS = {"/proc", "/sys", "/dev", "/run"}


def excluded(path: Path) -> bool:
    resolved = path.expanduser().resolve(strict=False)
    value = str(resolved).lower()
    if any(value == root or value.startswith(root + "/") for root in VIRTUAL_ROOTS):
        return True
    if resolved.name.lower() in SENSITIVE_NAMES or any(part.lower() in SENSITIVE_PARTS for part in resolved.parts):
        return True
    return any(term in value for term in ("secret", "credential", "password", "token"))


def _bounded(value: str) -> str:
    return " ".join(value.split())[:MAX_EXCERPT_CHARS]


def _command_output(arguments: list[str], timeout: float) -> str | None:
    try:
        result = subprocess.run(arguments, check=False, capture_output=True, text=True, timeout=timeout)
    except (OSError, subprocess.SubprocessError):
        return None
    return _bounded(result.stdout) if result.returncode == 0 and result.stdout else None


def extract(path: Path) -> dict[str, object] | None:
    """Return a bounded semantic excerpt for a safe, readable document."""
    if excluded(path) or not path.is_file():
        return None
    try:
        size = path.stat().st_size
    except OSError:
        return None
    if size > MAX_FILE_BYTES:
        return None
    suffix = path.suffix.lower()
    mime = mimetypes.guess_type(path.name)[0] or "application/octet-stream"
    excerpt: str | None = None
    kind: str | None = None
    if mime.startswith("text/") or suffix in TEXT_SUFFIXES:
        try:
            excerpt = _bounded(path.read_bytes()[:64 * 1024].decode("utf-8", errors="replace"))
            kind = "text"
        except OSError:
            return None
    elif suffix == ".pdf" and shutil.which("pdftotext"):
        excerpt = _command_output(["pdftotext", "-f", "1", "-l", "3", str(path), "-"], timeout=5)
        kind = "pdf"
    elif suffix in IMAGE_SUFFIXES and shutil.which("tesseract"):
        excerpt = _command_output(["tesseract", str(path), "stdout"], timeout=8)
        kind = "image"
    if not excerpt or not kind:
        return None
    digest = hashlib.sha256(path.read_bytes()[:64 * 1024]).hexdigest()
    return {"path": str(path.resolve()), "kind": kind, "mime": mime, "size_bytes": size, "sha256": digest, "excerpt": excerpt}
