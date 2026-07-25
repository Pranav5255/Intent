#!/usr/bin/env python3
"""Small GNOME/Ayatana AppIndicator for local Intent controls."""

from __future__ import annotations

import fcntl
import json
import os
import subprocess
import sys
from pathlib import Path
from urllib import request

LOCK_PATH = Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "intent-tray.lock"
EVENT_API = "http://127.0.0.1:9477/v1"
INTENT_API = "http://127.0.0.1:9478"


def fetch(url: str) -> object | None:
    try:
        with request.urlopen(url, timeout=1) as response:
            return json.loads(response.read().decode("utf-8"))
    except OSError:
        return None


def pause(paused: bool) -> None:
    body = json.dumps({"paused": paused}).encode("utf-8")
    try:
        request.urlopen(request.Request(f"{EVENT_API}/capture/pause", data=body, method="POST", headers={"Content-Type": "application/json"}), timeout=1)
    except OSError:
        pass


def open_app(*_: object) -> None:
    subprocess.Popen(["xdg-open", "http://127.0.0.1:9479"], start_new_session=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def main() -> int:
    # A lock prevents a second autostart invocation from duplicating the icon.
    LOCK_PATH.parent.mkdir(parents=True, exist_ok=True)
    lock = LOCK_PATH.open("w")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        return 0
    try:
        import gi
        gi.require_version("Gtk", "3.0")
        try:
            gi.require_version("AyatanaAppIndicator3", "0.1")
            from gi.repository import AyatanaAppIndicator3 as AppIndicator, GLib, Gtk
        except ValueError:
            gi.require_version("AppIndicator3", "0.1")
            from gi.repository import AppIndicator3 as AppIndicator, GLib, Gtk
    except (ImportError, ValueError):
        # Capture and notifications remain available on environments without a
        # traditional indicator host (for example GNOME Wayland test VMs).
        return 0

    indicator = AppIndicator.Indicator.new("intent", "dialog-information", AppIndicator.IndicatorCategory.APPLICATION_STATUS)
    indicator.set_status(AppIndicator.IndicatorStatus.ACTIVE)
    menu = Gtk.Menu()
    now_item = Gtk.MenuItem(label="Now: Working…")
    now_item.set_sensitive(False)
    yesterday = Gtk.MenuItem(label="Yesterday's intents")
    yesterday.connect("activate", open_app)
    open_item = Gtk.MenuItem(label="Open app")
    open_item.connect("activate", open_app)
    paused = {"value": False}
    pause_item = Gtk.MenuItem(label="Pause capture")

    def toggle_pause(*_: object) -> None:
        paused["value"] = not paused["value"]
        pause(paused["value"])
        pause_item.set_label("Resume capture" if paused["value"] else "Pause capture")

    pause_item.connect("activate", toggle_pause)
    quit_item = Gtk.MenuItem(label="Quit")
    quit_item.connect("activate", lambda *_: Gtk.main_quit())
    for item in (now_item, yesterday, pause_item, open_item, Gtk.SeparatorMenuItem(), quit_item):
        menu.append(item)
    menu.show_all()
    indicator.set_menu(menu)

    def update_now() -> bool:
        current = fetch(f"{INTENT_API}/intents/current")
        label = current.get("label") if isinstance(current, dict) and current.get("confidence", 0) >= 0.5 else "Working…"
        now_item.set_label(f"Now: {label}")
        return True

    update_now()
    GLib.timeout_add_seconds(60, update_now)
    Gtk.main()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
