#!/usr/bin/env python3
"""Watch approved workspaces and emit changed-path metadata only."""

from __future__ import annotations

import argparse
import json
import threading
import time
import uuid
from pathlib import Path
from typing import Callable
from urllib import request

from collectors.content import excluded, extract

from watchdog.events import FileSystemEvent, FileSystemEventHandler
from watchdog.observers import Observer


IGNORED_PARTS = {".git", ".hg", ".svn", "node_modules", ".venv", "venv", "dist", "build", "__pycache__"}


def is_within(path: Path, root: Path) -> bool:
    try:
        path.resolve().relative_to(root.resolve())
        return True
    except ValueError:
        return False


def should_capture(path: Path, root: Path, max_bytes: int = 25 * 1024 * 1024) -> bool:
    if excluded(path) or path.is_symlink() or not path.is_file() or not is_within(path, root):
        return False
    if any(part in IGNORED_PARTS for part in path.parts):
        return False
    try:
        return path.stat().st_size <= max_bytes
    except OSError:
        return False


def make_event(event_type: str, payload: dict[str, str]) -> dict[str, object]:
    return {
        "id": str(uuid.uuid4()),
        "ts": int(time.time()),
        "source": "filesystem",
        "type": event_type,
        "payload": payload,
    }


def post_event(endpoint: str, event: dict[str, object]) -> None:
    body = json.dumps(event, separators=(",", ":")).encode("utf-8")
    req = request.Request(endpoint, data=body, method="POST", headers={"Content-Type": "application/json"})
    with request.urlopen(req, timeout=1):
        pass


def safe_post(endpoint: str, event: dict[str, object]) -> None:
    try:
        post_event(endpoint, event)
    except OSError as exc:
        print(f"Intent OS workspace watcher cannot post event: {exc}", flush=True)


class WorkspaceHandler(FileSystemEventHandler):
    def __init__(self, root: Path, emit: Callable[[dict[str, object]], None], debounce_seconds: float = 2.0, capture_content: bool = False) -> None:
        self.root = root.resolve()
        self.emit = emit
        self.debounce_seconds = debounce_seconds
        self.capture_content = capture_content
        self._timers: dict[Path, threading.Timer] = {}
        self._lock = threading.Lock()

    def on_modified(self, event: FileSystemEvent) -> None:
        self._queue(Path(event.src_path))

    def on_created(self, event: FileSystemEvent) -> None:
        self._queue(Path(event.src_path))

    def _queue(self, path: Path) -> None:
        if path.is_dir() or not should_capture(path, self.root):
            return
        resolved = path.resolve()
        with self._lock:
            timer = self._timers.pop(resolved, None)
            if timer:
                timer.cancel()
            timer = threading.Timer(self.debounce_seconds, self._emit, args=(resolved,))
            timer.daemon = True
            self._timers[resolved] = timer
            timer.start()

    def _emit(self, path: Path) -> None:
        with self._lock:
            self._timers.pop(path, None)
        if should_capture(path, self.root):
            self.emit(make_event("file_modify", {"path": str(path), "workspace": str(self.root)}))
            if self.capture_content:
                content = extract(path)
                if content:
                    self.emit(make_event("file_content", {**content, "workspace": str(self.root)}))


def run(endpoint: str, workspaces: list[Path], capture_content: bool = False) -> None:
    observer = Observer()
    for workspace in workspaces:
        root = workspace.expanduser().resolve()
        if not root.is_dir():
            raise ValueError(f"workspace is not a directory: {workspace}")
        safe_post(endpoint, make_event("workspace_seen", {"workspace": str(root)}))
        observer.schedule(WorkspaceHandler(root, lambda item: safe_post(endpoint, item), capture_content=capture_content), str(root), recursive=True)
    observer.start()
    try:
        while True:
            time.sleep(1)
    finally:
        observer.stop()
        observer.join()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", default="http://127.0.0.1:9477/v1/event")
    parser.add_argument("--workspace", type=Path, action="append", required=True, help="approved root; repeat for each workspace")
    args = parser.parse_args()
    try:
        run(args.endpoint, args.workspace)
    except ValueError as exc:
        parser.error(str(exc))


if __name__ == "__main__":
    main()
