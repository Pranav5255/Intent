#!/usr/bin/env python3
"""Per-user control plane for Intent OS on Ubuntu."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from urllib import request

INSTALL_ROOT = Path(os.environ.get("INTENT_OS_INSTALL_ROOT", "/opt/intent-os"))
SOURCE_ROOT = Path(__file__).resolve().parents[1]
IMPORT_ROOT = SOURCE_ROOT if (SOURCE_ROOT / "event_server").is_dir() else INSTALL_ROOT
if str(IMPORT_ROOT) not in sys.path:
    sys.path.insert(0, str(IMPORT_ROOT))

from event_server import detailed_capture
from tools import workspaces


AUTOSTART_SOURCE = INSTALL_ROOT / "packaging" / "debian" / "autostart" / "intent-os.desktop"
VSIX_PATH = INSTALL_ROOT / "integrations" / "vscode-extension" / "dist" / "intent-os-vscode.vsix"
SHELL_SOURCES = {
    "bash": INSTALL_ROOT / "shell" / "intent-os.bash",
    "zsh": INSTALL_ROOT / "shell" / "intent-os.zsh",
}
MARKER_START = "# >>> Intent OS shell integration >>>"
MARKER_END = "# <<< Intent OS shell integration <<<"
UNIT_NAMES = ("intent-os-server.service", "intent-os-x11-tracker.service", "intent-os-workspace-watch.service")


def run_systemctl(*arguments: str) -> None:
    subprocess.run(["systemctl", "--user", *arguments], check=True)


def import_graphical_environment() -> None:
    names = ["DISPLAY", "XAUTHORITY", "XDG_SESSION_TYPE", "DBUS_SESSION_BUS_ADDRESS"]
    present = [name for name in names if os.environ.get(name)]
    if present:
        run_systemctl("import-environment", *present)


def enable_services() -> None:
    import_graphical_environment()
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
    subprocess.run([editor, "--install-extension", str(VSIX_PATH)], check=True)


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


def fetch_json(url: str) -> dict[str, object]:
    with request.urlopen(url, timeout=1) as response:
        return json.loads(response.read().decode("utf-8"))


def post_json(url: str) -> dict[str, object]:
    req = request.Request(url, data=b"{}", method="POST", headers={"Content-Type": "application/json"})
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


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("enable")
    subparsers.add_parser("disable")
    subparsers.add_parser("session-start")
    subparsers.add_parser("status")
    export = subparsers.add_parser("export-day")
    export.add_argument("--date", required=True, help="YYYY-MM-DD")
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
    detailed.add_argument("source", choices=("editor", "browser"))
    detailed.add_argument("action", choices=("enable", "disable"))
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
        elif args.command == "purge-detailed":
            print(json.dumps(post_json("http://127.0.0.1:9477/v1/detailed-capture/purge"), indent=2))
    except (OSError, RuntimeError, subprocess.CalledProcessError) as exc:
        print(f"Intent OS: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
