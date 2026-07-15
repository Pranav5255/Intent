#!/usr/bin/env python3
"""Emit X11 foreground-window changes to the local Intent OS server."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import time
import uuid
from dataclasses import dataclass
from typing import Callable
from urllib import request


class X11Unavailable(RuntimeError):
    """Raised when a foreground window cannot be inspected."""


@dataclass(frozen=True)
class ActiveWindow:
    window_id: str
    app: str
    title: str

    @property
    def signature(self) -> tuple[str, str, str]:
        return self.window_id, self.app, self.title


FRIENDLY_APP_NAMES = {
    "firefox": "firefox",
    "google-chrome": "google-chrome",
    "code": "code",
    "gnome-terminal-server": "gnome-terminal",
    "gnome-terminal": "gnome-terminal",
}


def parse_wm_class(raw: str) -> str:
    """Extract a stable WM_CLASS identifier from xprop output."""
    quoted = re.findall(r'"([^\"]+)"', raw)
    candidate = (quoted[-1] if quoted else raw.rsplit(",", maxsplit=1)[-1]).strip().lower()
    return FRIENDLY_APP_NAMES.get(candidate, candidate or "unknown")


def _run(command: list[str]) -> str:
    try:
        return subprocess.run(command, capture_output=True, check=True, text=True, timeout=1).stdout.strip()
    except (FileNotFoundError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as exc:
        raise X11Unavailable("failed to inspect active X11 window") from exc


def get_active_window(run: Callable[[list[str]], str] = _run) -> ActiveWindow:
    window_id = run(["xdotool", "getactivewindow"])
    if not window_id:
        raise X11Unavailable("no active X11 window")
    title = run(["xdotool", "getwindowname", window_id])[:512]
    wm_class = run(["xprop", "-id", window_id, "WM_CLASS"])
    return ActiveWindow(window_id=window_id, app=parse_wm_class(wm_class), title=title)


def build_event(window: ActiveWindow) -> dict[str, object]:
    return {
        "id": str(uuid.uuid4()),
        "ts": int(time.time()),
        "source": "linux",
        "type": "app_focus",
        "payload": {"app": window.app, "title": window.title, "window_id": window.window_id},
    }


def post_event(endpoint: str, event: dict[str, object]) -> None:
    body = json.dumps(event, separators=(",", ":")).encode("utf-8")
    req = request.Request(endpoint, data=body, method="POST", headers={"Content-Type": "application/json"})
    with request.urlopen(req, timeout=1):
        pass


def run_tracker(endpoint: str, interval: float, once: bool = False) -> int:
    missing = [name for name in ("xdotool", "xprop") if not shutil.which(name)]
    if missing:
        print("Intent OS X11 tracker disabled: missing " + ", ".join(missing), flush=True)
        return 2
    if os.environ.get("XDG_SESSION_TYPE", "").lower() == "wayland":
        print("Intent OS X11 tracker disabled: Wayland session detected", flush=True)
        return 2

    previous: tuple[str, str, str] | None = None
    while True:
        try:
            window = get_active_window()
            if window.signature != previous:
                post_event(endpoint, build_event(window))
                previous = window.signature
        except X11Unavailable as exc:
            print(f"Intent OS X11 tracker: {exc}", flush=True)
        except OSError as exc:
            print(f"Intent OS X11 tracker cannot post event: {exc}", flush=True)
        if once:
            return 0
        time.sleep(interval)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", default="http://127.0.0.1:9477/v1/event")
    parser.add_argument("--interval", type=float, default=2.0)
    parser.add_argument("--once", action="store_true")
    args = parser.parse_args()
    if args.interval <= 0:
        parser.error("--interval must be positive")
    raise SystemExit(run_tracker(args.endpoint, args.interval, args.once))


if __name__ == "__main__":
    main()
