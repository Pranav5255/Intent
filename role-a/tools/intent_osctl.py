#!/usr/bin/env python3
"""Per-user control plane for Intent OS on Ubuntu."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Any
from urllib import error, request

INSTALL_ROOT = Path(os.environ.get("INTENT_OS_INSTALL_ROOT", "/opt/intent-os"))
SOURCE_ROOT = Path(__file__).resolve().parents[1]
IMPORT_ROOT = SOURCE_ROOT if (SOURCE_ROOT / "event_server").is_dir() else INSTALL_ROOT
if str(IMPORT_ROOT) not in sys.path:
    sys.path.insert(0, str(IMPORT_ROOT))

ROLE_B_ROOT = IMPORT_ROOT / "role-b"
if not ROLE_B_ROOT.is_dir():
    ROLE_B_ROOT = SOURCE_ROOT.parent / "role-b"
ROLE_B_PYTHON = ROLE_B_ROOT / ".venv" / "bin" / "python"

from event_server import detailed_capture
from tools import filesystem_capture, workspaces


AUTOSTART_SOURCE = IMPORT_ROOT / "packaging" / "debian" / "autostart" / "intent-os.desktop"
VSIX_PATH = IMPORT_ROOT / "integrations" / "vscode-extension" / "dist" / "intent-os-vscode.vsix"
SHELL_SOURCES = {
    "bash": IMPORT_ROOT / "shell" / "intent-os.bash",
    "zsh": IMPORT_ROOT / "shell" / "intent-os.zsh",
}
SYSTEMD_SOURCE = IMPORT_ROOT / "packaging" / "systemd"
MARKER_START = "# >>> Intent OS shell integration >>>"
MARKER_END = "# <<< Intent OS shell integration <<<"
UNIT_NAMES = (
    "intent-os-server.service",
    "intent-os-role-b.service",
    "intent-os-x11-tracker.service",
    "intent-os-workspace-watch.service",
)
INTENT_API = "http://127.0.0.1:9478"
EVENT_API = "http://127.0.0.1:9477"


def run_systemctl(*arguments: str) -> None:
    subprocess.run(["systemctl", "--user", *arguments], check=True)


def import_graphical_environment() -> None:
    names = ["DISPLAY", "XAUTHORITY", "XDG_SESSION_TYPE", "DBUS_SESSION_BUS_ADDRESS"]
    present = [name for name in names if os.environ.get(name)]
    if present:
        run_systemctl("import-environment", *present)


def install_systemd_units() -> None:
    destination_dir = Path.home() / ".config" / "systemd" / "user"
    destination_dir.mkdir(parents=True, exist_ok=True)
    python_executable = sys.executable
    install_root = str(IMPORT_ROOT)
    for unit_name in UNIT_NAMES:
        source = SYSTEMD_SOURCE / unit_name
        if not source.is_file():
            raise RuntimeError(f"systemd unit is unavailable: {source}")
        contents = source.read_text(encoding="utf-8")
        contents = contents.replace("@INTENT_OS_INSTALL_ROOT@", install_root)
        contents = contents.replace("@INTENT_OS_PYTHON@", python_executable)
        contents = contents.replace("@INTENT_OS_ROLE_B_ROOT@", str(ROLE_B_ROOT))
        contents = contents.replace("@INTENT_OS_ROLE_B_PYTHON@", str(ROLE_B_PYTHON))
        (destination_dir / unit_name).write_text(contents, encoding="utf-8")


def enable_services() -> None:
    import_graphical_environment()
    install_systemd_units()
    run_systemctl("daemon-reload")
    run_systemctl("enable", "--now", *UNIT_NAMES)
    install_autostart()


def disable_services() -> None:
    run_systemctl("disable", "--now", *UNIT_NAMES)
    autostart = Path.home() / ".config" / "autostart" / "intent-os.desktop"
    autostart.unlink(missing_ok=True)


def session_start() -> None:
    import_graphical_environment()
    run_systemctl("start", *UNIT_NAMES)
    start_tray()


def start_tray() -> None:
    tray = IMPORT_ROOT / "tools" / "intent_os_tray.py"
    if not tray.is_file():
        raise RuntimeError(f"tray helper is unavailable: {tray}")
    subprocess.Popen([sys.executable, str(tray)], start_new_session=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def install_autostart() -> None:
    destination = Path.home() / ".config" / "autostart" / "intent-os.desktop"
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(AUTOSTART_SOURCE, destination)


def install_editor_extension(command: str, label: str) -> None:
    editor = shutil.which(command)
    if not editor:
        raise RuntimeError(f"{label} CLI {command} is unavailable")
    if not VSIX_PATH.is_file():
        raise RuntimeError(f"bundled VSIX is unavailable: {VSIX_PATH}")
    subprocess.run([editor, "--install-extension", str(VSIX_PATH), "--force"], check=True)


def uninstall_editor_extension(command: str, label: str) -> None:
    editor = shutil.which(command)
    if not editor:
        raise RuntimeError(f"{label} CLI {command} is unavailable")
    subprocess.run([editor, "--uninstall-extension", "intent-os.intent-os-vscode"], check=True)


def install_vscode() -> None:
    install_editor_extension("code", "VS Code")


def uninstall_vscode() -> None:
    uninstall_editor_extension("code", "VS Code")


def install_cursor() -> None:
    install_editor_extension("cursor", "Cursor")


def uninstall_cursor() -> None:
    uninstall_editor_extension("cursor", "Cursor")


def update_workspace(path: Path, enabled: bool) -> dict[str, list[str]]:
    config = workspaces.load()
    config = workspaces.add(path, config) if enabled else workspaces.remove(path, config)
    workspaces.save(config)
    subprocess.run(["systemctl", "--user", "try-restart", "intent-os-workspace-watch.service"], check=False)
    return config


def shell_rc(shell: str, home: Path | None = None) -> Path:
    home = home or Path.home()
    return home / (".bashrc" if shell == "bash" else ".zshrc")


def remove_shell_block(contents: str) -> str:
    before, marker, after = contents.partition(MARKER_START)
    if not marker:
        return contents
    _, end, remainder = after.partition(MARKER_END)
    if not end:
        return contents
    return (before.rstrip() + "\n" + remainder.lstrip()).rstrip() + "\n"


def update_shell_integration(shell: str, enabled: bool, home: Path | None = None) -> Path:
    if shell not in SHELL_SOURCES:
        raise ValueError(f"unsupported shell: {shell}")
    rc_path = shell_rc(shell, home)
    contents = rc_path.read_text(encoding="utf-8") if rc_path.exists() else ""
    contents = remove_shell_block(contents)
    if enabled:
        block = f"{MARKER_START}\nsource {SHELL_SOURCES[shell]}\n{MARKER_END}\n"
        contents = (contents.rstrip() + "\n\n" + block) if contents.strip() else block
    rc_path.parent.mkdir(parents=True, exist_ok=True)
    rc_path.write_text(contents, encoding="utf-8")
    return rc_path


def fetch_json(url: str) -> Any:
    with request.urlopen(url, timeout=1) as response:
        return json.loads(response.read().decode("utf-8"))


def post_json(url: str, payload: dict[str, object] | None = None) -> dict[str, object]:
    req = request.Request(url, data=json.dumps(payload or {}).encode("utf-8"), method="POST", headers={"Content-Type": "application/json"})
    with request.urlopen(req, timeout=1) as response:
        return json.loads(response.read().decode("utf-8"))


def command_status() -> int:
    try:
        print(json.dumps(fetch_json("http://127.0.0.1:9477/v1/status"), indent=2))
        return 0
    except OSError as exc:
        print(f"Intent OS server is unavailable: {exc}", file=sys.stderr)
        return 1


def command_export(date: str) -> int:
    try:
        print(json.dumps(fetch_json(f"http://127.0.0.1:9477/v1/export/day?date={date}"), indent=2))
        return 0
    except OSError as exc:
        print(f"Intent OS export failed: {exc}", file=sys.stderr)
        return 1


def wait_for_json(url: str, timeout: float = 30) -> Any:
    """Wait through graphical-session startup without polling forever."""
    deadline = time.monotonic() + timeout
    last_error: OSError | None = None
    while time.monotonic() < deadline:
        try:
            return fetch_json(url)
        except (OSError, error.URLError, ValueError) as exc:
            last_error = exc
            time.sleep(1)
    raise RuntimeError(f"service did not become ready: {url} ({last_error})")


def command_notify_yesterday() -> int:
    try:
        wait_for_json(f"{EVENT_API}/healthz")
        intents = wait_for_json(f"{INTENT_API}/intents/yesterday")
    except RuntimeError as exc:
        print(f"Intent OS notification skipped: {exc}", file=sys.stderr)
        return 0
    if not isinstance(intents, list) or not intents:
        return 0
    top = intents[0]
    if not isinstance(top, dict):
        return 0
    label = str(top.get("label", "your work"))
    summary = str(top.get("summary", "Open Intent OS to continue."))
    message = f"Good morning. Yesterday you were {label.lower()}.\n{summary}"
    notify = shutil.which("notify-send")
    if not notify:
        print("Intent OS notification skipped: notify-send is unavailable", file=sys.stderr)
        return 0
    try:
        result = subprocess.run(
            [notify, "--app-name=Intent OS", "--icon=dialog-information", "--action=default=Open", "Intent OS", message],
            check=False,
            capture_output=True,
            text=True,
        )
        if result.returncode == 0 and result.stdout.strip() == "default":
            opener = shutil.which("xdg-open")
            if opener:
                subprocess.Popen([opener, "http://127.0.0.1:9479"], start_new_session=True)
    except OSError as exc:
        print(f"Intent OS notification skipped: {exc}", file=sys.stderr)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("enable")
    subparsers.add_parser("disable")
    subparsers.add_parser("session-start")
    subparsers.add_parser("status")
    export = subparsers.add_parser("export-day")
    export.add_argument("--date", required=True, help="YYYY-MM-DD")
    subparsers.add_parser("notify-yesterday", help="show the morning intent notification after local services start")
    subparsers.add_parser("tray", help="start the local GNOME tray indicator")
    vscode = subparsers.add_parser("vscode")
    vscode.add_argument("action", choices=("install", "uninstall"))
    cursor = subparsers.add_parser("cursor")
    cursor.add_argument("action", choices=("install", "uninstall"))
    shell = subparsers.add_parser("shell")
    shell.add_argument("action", choices=("enable", "disable"))
    shell.add_argument("--shell", choices=("bash", "zsh"), default=Path(os.environ.get("SHELL", "bash")).name)
    workspace = subparsers.add_parser("workspace")
    workspace.add_argument("action", choices=("add", "remove", "list"))
    workspace.add_argument("path", type=Path, nargs="?")
    detailed = subparsers.add_parser("detailed", help="control consented detailed capture")
    detailed.add_argument("source", choices=("editor", "browser", "filesystem"))
    detailed.add_argument("action", choices=("enable", "disable"))
    browser_context = subparsers.add_parser("browser-context", help="control bounded public-post context on explicit browser actions")
    browser_context.add_argument("action", choices=("enable", "disable"))
    filesystem = subparsers.add_parser("filesystem", help="control broad user-readable filesystem observation")
    filesystem.add_argument("action", choices=("enable-all-accessible", "disable-all-accessible"))
    subparsers.add_parser("purge-detailed", help="delete locally stored detailed events")
    args = parser.parse_args()

    try:
        if args.command == "enable":
            enable_services()
        elif args.command == "disable":
            disable_services()
        elif args.command == "session-start":
            session_start()
        elif args.command == "status":
            return command_status()
        elif args.command == "export-day":
            return command_export(args.date)
        elif args.command == "notify-yesterday":
            return command_notify_yesterday()
        elif args.command == "tray":
            start_tray()
        elif args.command == "vscode":
            install_vscode() if args.action == "install" else uninstall_vscode()
        elif args.command == "cursor":
            install_cursor() if args.action == "install" else uninstall_cursor()
        elif args.command == "shell":
            print(update_shell_integration(args.shell, args.action == "enable"))
        elif args.command == "workspace":
            if args.action == "list":
                print(json.dumps(workspaces.load(), indent=2))
            elif args.path is None:
                parser.error("workspace add/remove requires a path")
            else:
                print(json.dumps(update_workspace(args.path, args.action == "add"), indent=2))
        elif args.command == "detailed":
            print(json.dumps(detailed_capture.set_enabled(args.source, args.action == "enable"), indent=2))
        elif args.command == "browser-context":
            print(json.dumps(detailed_capture.set_browser_context_enabled(args.action == "enable"), indent=2))
        elif args.command == "filesystem":
            config = filesystem_capture.load()
            config["all_accessible"] = args.action == "enable-all-accessible"
            filesystem_capture.save(config)
            subprocess.run(["systemctl", "--user", "try-restart", "intent-os-workspace-watch.service"], check=False)
            print(json.dumps(config, indent=2))
        elif args.command == "purge-detailed":
            print(json.dumps(post_json("http://127.0.0.1:9477/v1/detailed-capture/purge"), indent=2))
    except (OSError, RuntimeError, subprocess.CalledProcessError) as exc:
        print(f"Intent OS: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
