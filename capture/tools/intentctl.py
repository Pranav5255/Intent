#!/usr/bin/env python3
"""Per-user control plane for Intent on Ubuntu."""

from __future__ import annotations

import argparse
import json
import os
import shlex
import shutil
import sqlite3
import subprocess
import sys
import time
from datetime import date as calendar_date
from pathlib import Path
from typing import Any
from urllib import error, request
from urllib.parse import urlencode

INSTALL_ROOT = Path(os.environ.get("INTENT_INSTALL_ROOT", "/opt/intent"))
SOURCE_ROOT = Path(__file__).resolve().parents[1]
IMPORT_ROOT = SOURCE_ROOT if (SOURCE_ROOT / "event_server").is_dir() else INSTALL_ROOT
if str(IMPORT_ROOT) not in sys.path:
    sys.path.insert(0, str(IMPORT_ROOT))

ENGINE_ROOT = IMPORT_ROOT / "engine"
if not ENGINE_ROOT.is_dir():
    ENGINE_ROOT = SOURCE_ROOT.parent / "engine"
ENGINE_PYTHON = ENGINE_ROOT / ".venv" / "bin" / "python"

from event_server import detailed_capture
from tools import filesystem_capture, workspaces


AUTOSTART_SOURCE = IMPORT_ROOT / "packaging" / "debian" / "autostart" / "intent.desktop"
VSIX_PATH = IMPORT_ROOT / "integrations" / "vscode-extension" / "dist" / "intent-vscode.vsix"
SHELL_SOURCES = {
    "bash": IMPORT_ROOT / "shell" / "intent.bash",
    "zsh": IMPORT_ROOT / "shell" / "intent.zsh",
}
SYSTEMD_SOURCE = IMPORT_ROOT / "packaging" / "systemd"
MARKER_START = "# >>> Intent shell integration >>>"
MARKER_END = "# <<< Intent shell integration <<<"
UNIT_NAMES = (
    "intent-backend.target",
    "intent-server.service",
    "intent-engine.service",
    "intent-x11-tracker.service",
    "intent-workspace-watch.service",
    "intent-pipeline.service",
    "intent-pipeline.timer",
)
BACKEND_TARGET = "intent-backend.target"
STATUS_UNITS = (
    "intent-server.service",
    "intent-engine.service",
    "intent-pipeline.service",
    "intent-pipeline.timer",
)
BACKEND_COMPONENT_UNITS = (
    "intent-server.service",
    "intent-engine.service",
    "intent-x11-tracker.service",
    "intent-workspace-watch.service",
    "intent-pipeline.service",
    "intent-pipeline.timer",
)
SCHEDULER_OUTCOMES = {"success", "unchanged", "role_a_unavailable", "pipeline_error"}
INTENT_API = "http://127.0.0.1:9478"
EVENT_API = "http://127.0.0.1:9477"
ROLE_C_PREVIEW_URL = "http://127.0.0.1:9479/preview"
EXTENSION_ID = "intent.intent-vscode"
LEGACY_EXTENSION_ID = "intent-os.intent-os-vscode"
FIREFOX_POLICY_FILE = Path("/etc/firefox/policies/policies.json")
CONFIG_DIR = Path.home() / ".config" / "intent"
DATA_DIR = Path.home() / ".local" / "share" / "intent"
LEGACY_CONFIG_DIR = Path.home() / ".config" / "intent-os"
LEGACY_DATA_DIR = Path.home() / ".local" / "share" / "intent-os"


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
        contents = contents.replace("@INTENT_INSTALL_ROOT@", install_root)
        contents = contents.replace("@INTENT_PYTHON@", python_executable)
        contents = contents.replace("@INTENT_ENGINE_ROOT@", str(ENGINE_ROOT))
        contents = contents.replace("@INTENT_ENGINE_PYTHON@", str(ENGINE_PYTHON))
        (destination_dir / unit_name).write_text(contents, encoding="utf-8")


def enable_services() -> None:
    import_graphical_environment()
    install_systemd_units()
    run_systemctl("daemon-reload")
    # Remove pre-target startup links from older installations before the
    # backend target becomes the sole default-target entry point.
    run_systemctl("disable", *BACKEND_COMPONENT_UNITS)
    run_systemctl("enable", "--now", BACKEND_TARGET)
    install_autostart()
    install_companions()


def disable_services() -> None:
    run_systemctl("disable", "--now", BACKEND_TARGET)
    run_systemctl("disable", *BACKEND_COMPONENT_UNITS)
    autostart = Path.home() / ".config" / "autostart" / "intent.desktop"
    autostart.unlink(missing_ok=True)


def session_start() -> None:
    import_graphical_environment()
    run_systemctl("start", BACKEND_TARGET)
    start_tray()


def start_tray() -> None:
    tray = IMPORT_ROOT / "tools" / "intent_tray.py"
    if not tray.is_file():
        raise RuntimeError(f"tray helper is unavailable: {tray}")
    subprocess.Popen([sys.executable, str(tray)], start_new_session=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def install_autostart() -> None:
    destination = Path.home() / ".config" / "autostart" / "intent.desktop"
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(AUTOSTART_SOURCE, destination)


def install_editor_extension(command: str, label: str) -> None:
    editor = shutil.which(command)
    if not editor:
        raise RuntimeError(f"{label} CLI {command} is unavailable")
    if not VSIX_PATH.is_file():
        raise RuntimeError(f"bundled VSIX is unavailable: {VSIX_PATH}")
    subprocess.run([editor, "--install-extension", str(VSIX_PATH), "--force"], check=True)


def uninstall_editor_extension(command: str, label: str, *, legacy: bool = False) -> None:
    editor = shutil.which(command)
    if not editor:
        raise RuntimeError(f"{label} CLI {command} is unavailable")
    for extension_id in [EXTENSION_ID, *( [LEGACY_EXTENSION_ID] if legacy else [] )]:
        subprocess.run([editor, "--uninstall-extension", extension_id], check=False)


def editor_extension_installed(command: str, extension_id: str = EXTENSION_ID) -> bool:
    editor = shutil.which(command)
    if not editor:
        return False
    result = subprocess.run(
        [editor, "--list-extensions"],
        check=False,
        capture_output=True,
        text=True,
    )
    return extension_id in result.stdout.splitlines()


def firefox_policy_present() -> bool:
    try:
        contents = FIREFOX_POLICY_FILE.read_text(encoding="utf-8")
    except OSError:
        return False
    return "intent@local" in contents or "intent-firefox.xpi" in contents


def install_companions() -> None:
    for install_fn, label in ((install_vscode, "VS Code"), (install_cursor, "Cursor")):
        try:
            install_fn()
        except (OSError, RuntimeError, subprocess.CalledProcessError) as exc:
            print(f"Intent: {label} companion install skipped: {exc}", file=sys.stderr)


def remove_systemd_artifacts() -> None:
    unit_dir = Path.home() / ".config" / "systemd" / "user"
    for pattern in ("intent-*", "intent-os-*"):
        for path in unit_dir.glob(pattern):
            if path.is_dir():
                shutil.rmtree(path, ignore_errors=True)
            else:
                path.unlink(missing_ok=True)


def kill_running_processes() -> None:
    patterns = (
        "/opt/intent/desktop/electron",
        "/opt/intent-os/role-c/electron",
        "intent_tray.py",
        "intent_os_tray.py",
    )
    for pattern in patterns:
        subprocess.run(["pkill", "-f", pattern], check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def purge_user_paths(*, keep_data: bool, legacy: bool) -> None:
    if not keep_data:
        shutil.rmtree(CONFIG_DIR, ignore_errors=True)
        shutil.rmtree(DATA_DIR, ignore_errors=True)
        if legacy:
            shutil.rmtree(LEGACY_CONFIG_DIR, ignore_errors=True)
            shutil.rmtree(LEGACY_DATA_DIR, ignore_errors=True)
    autostart = Path.home() / ".config" / "autostart"
    for name in ("intent.desktop", "intent-os.desktop"):
        (autostart / name).unlink(missing_ok=True)


def purge_system_package(*, legacy: bool) -> None:
    if os.geteuid() != 0:
        return
    for package in ("intent", *(["intent-os"] if legacy else [])):
        subprocess.run(["apt-get", "purge", "-y", package], check=False)


def command_uninstall(*, keep_data: bool, skip_package: bool, legacy: bool) -> int:
    try:
        disable_services()
    except subprocess.CalledProcessError:
        pass
    for shell_name in ("bash", "zsh"):
        try:
            update_shell_integration(shell_name, enabled=False)
        except (OSError, ValueError):
            pass
    for uninstall_fn in (uninstall_vscode, uninstall_cursor):
        try:
            uninstall_fn()
        except (OSError, RuntimeError, subprocess.CalledProcessError):
            pass
    if legacy:
        for command, label in (("code", "VS Code"), ("cursor", "Cursor")):
            try:
                uninstall_editor_extension(command, label, legacy=True)
            except (OSError, RuntimeError, subprocess.CalledProcessError):
                pass
    remove_systemd_artifacts()
    subprocess.run(["systemctl", "--user", "daemon-reload"], check=False)
    purge_user_paths(keep_data=keep_data, legacy=legacy)
    kill_running_processes()
    if not skip_package and shutil.which("apt-get"):
        purge_system_package(legacy=legacy)
    print("Intent uninstalled.", file=sys.stderr)
    print("Open a new terminal to clear shell integration from memory.", file=sys.stderr)
    return 0


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
    subprocess.run(["systemctl", "--user", "try-restart", "intent-workspace-watch.service"], check=False)
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
    event_status: Any = None
    event_api_available = True
    try:
        event_status = fetch_json("http://127.0.0.1:9477/v1/status")
    except OSError:
        event_api_available = False
    print(json.dumps({
        "event_server": event_status,
        "services": {unit: systemd_unit_state(unit) for unit in STATUS_UNITS},
        "pipeline": {
            **scheduler_state(),
            "next_timer_activation": systemd_timer_next_activation(),
        },
        "companions": {
            "vscode_installed": editor_extension_installed("code") or editor_extension_installed("code", LEGACY_EXTENSION_ID),
            "cursor_installed": editor_extension_installed("cursor") or editor_extension_installed("cursor", LEGACY_EXTENSION_ID),
            "firefox_policy_present": firefox_policy_present(),
        },
    }, indent=2))
    return 0 if event_api_available else 1


def systemd_unit_state(unit: str) -> str:
    """Return an operational state without making `status` fail when systemd is unavailable."""

    try:
        result = subprocess.run(
            ["systemctl", "--user", "is-active", unit],
            check=False,
            capture_output=True,
            text=True,
        )
    except OSError:
        return "unknown"
    state = result.stdout.strip()
    return state if state else "unknown"


def systemd_timer_next_activation() -> str | None:
    try:
        result = subprocess.run(
            [
                "systemctl", "--user", "show", "intent-pipeline.timer",
                "--property=NextElapseUSecRealtime", "--value",
            ],
            check=False,
            capture_output=True,
            text=True,
        )
    except OSError:
        return None
    value = result.stdout.strip()
    return value or None


def scheduler_state() -> dict[str, str | int | None]:
    """Read only Role B's fixed scheduler metadata; never surface arbitrary values."""

    database_path = Path(os.environ.get("ROLE_B_DB_PATH", Path.home() / ".local" / "share" / "intent" / "intents.db"))
    today = calendar_date.today().isoformat()
    try:
        connection = sqlite3.connect(f"file:{database_path}?mode=ro", uri=True)
        try:
            rows = dict(connection.execute(
                "SELECT key, value FROM store_metadata WHERE key IN (?, ?, ?)",
                ("scheduled_ingest_last_completed_date", "scheduled_ingest_last_outcome", f"ingest_day:{today}"),
            ).fetchall())
        finally:
            connection.close()
    except (OSError, sqlite3.Error):
        return {
            "last_completed_date": None,
            "last_outcome": None,
            "today_content_hash": None,
            "today_event_count": None,
        }

    completed_date = rows.get("scheduled_ingest_last_completed_date")
    try:
        calendar_date.fromisoformat(completed_date) if isinstance(completed_date, str) else None
    except ValueError:
        completed_date = None
    outcome = rows.get("scheduled_ingest_last_outcome")

    today_content_hash: str | None = None
    today_event_count: int | None = None
    ingest_raw = rows.get(f"ingest_day:{today}")
    if isinstance(ingest_raw, str):
        try:
            ingest_state = json.loads(ingest_raw)
            if isinstance(ingest_state.get("content_hash"), str):
                today_content_hash = ingest_state["content_hash"]
            if isinstance(ingest_state.get("event_count"), int):
                today_event_count = ingest_state["event_count"]
        except json.JSONDecodeError:
            pass

    return {
        "last_completed_date": completed_date if isinstance(completed_date, str) else None,
        "last_outcome": outcome if outcome in SCHEDULER_OUTCOMES else None,
        "today_content_hash": today_content_hash,
        "today_event_count": today_event_count,
        "incremental_ingest_enabled": os.environ.get("ENABLE_INCREMENTAL_INGEST", "false").strip().lower()
        in {"true", "1", "yes"},
    }


def command_export(date: str) -> int:
    try:
        print(json.dumps(fetch_json(f"http://127.0.0.1:9477/v1/export/day?date={date}"), indent=2))
        return 0
    except OSError as exc:
        print(f"Intent export failed: {exc}", file=sys.stderr)
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


def command_wait_ready() -> int:
    """Wait for the two local APIs used by the bundled desktop overlay."""

    try:
        wait_for_json(f"{EVENT_API}/healthz")
        wait_for_json(f"{INTENT_API}/healthz")
    except RuntimeError as exc:
        print(f"Intent: {exc}", file=sys.stderr)
        return 1
    return 0


def preview_url(intent_id: str) -> str:
    """Return the Role C preview link for one stored intent without restoring it."""

    return f"{ROLE_C_PREVIEW_URL}?{urlencode({'intent_id': intent_id, 'restore_scope': 'same_project'})}"


def notification_subject(intent: dict[str, object]) -> str:
    """Choose a short project-first notification subject from safe stored fields."""

    tags = intent.get("tags")
    if isinstance(tags, list):
        for tag in tags:
            if isinstance(tag, str) and tag.casefold().startswith("project:"):
                project = tag.split(":", 1)[1].strip()
                if project:
                    return project.replace("-", " ").replace("_", " ").title()
    label = intent.get("label")
    return str(label).strip() or "your work"


def launch_preview(url: str) -> bool:
    """Invoke only an explicitly configured Intent preview client."""

    raw_command = os.environ.get("INTENT_PREVIEW_COMMAND", "").strip()
    if not raw_command:
        return False
    try:
        command = shlex.split(raw_command)
    except ValueError:
        return False
    if not command or not shutil.which(command[0]):
        return False
    subprocess.Popen(
        [*command, url],
        start_new_session=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    return True


def command_notify_yesterday() -> int:
    try:
        wait_for_json(f"{EVENT_API}/healthz")
        intents = wait_for_json(f"{INTENT_API}/intents/yesterday")
    except RuntimeError as exc:
        print(f"Intent notification skipped: {exc}", file=sys.stderr)
        return 0
    if not isinstance(intents, list) or not intents:
        return 0
    top = intents[0]
    if not isinstance(top, dict):
        return 0
    summary = str(top.get("summary", "Open Intent to continue."))
    intent_id = top.get("id")
    if not isinstance(intent_id, str) or not intent_id.strip():
        return 0
    subject = notification_subject(top)
    message = summary
    notify = shutil.which("notify-send")
    if not notify:
        print("Intent notification skipped: notify-send is unavailable", file=sys.stderr)
        return 0
    try:
        result = subprocess.run(
            [notify, "--app-name=Intent", "--icon=dialog-information", "--action=default=Preview", f"Continue {subject}?", message],
            check=False,
            capture_output=True,
            text=True,
        )
        if result.returncode == 0 and result.stdout.strip() == "default":
            if not launch_preview(preview_url(intent_id)):
                print("Intent preview launcher unavailable", file=sys.stderr)
    except OSError as exc:
        print(f"Intent notification skipped: {exc}", file=sys.stderr)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("enable")
    subparsers.add_parser("disable")
    subparsers.add_parser("session-start")
    subparsers.add_parser("wait-ready", help="wait for the local Role A and Role B APIs")
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
    uninstall = subparsers.add_parser("uninstall", help="remove Intent services, companions, config, and optionally the system package")
    uninstall.add_argument("--keep-data", action="store_true", help="keep ~/.config/intent and ~/.local/share/intent databases")
    uninstall.add_argument("--skip-package", action="store_true", help="do not run apt purge")
    uninstall.add_argument("--legacy", action="store_true", help="also remove intent-os package paths and extension IDs")
    args = parser.parse_args()

    try:
        if args.command == "enable":
            enable_services()
        elif args.command == "disable":
            disable_services()
        elif args.command == "session-start":
            session_start()
        elif args.command == "wait-ready":
            return command_wait_ready()
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
            subprocess.run(["systemctl", "--user", "try-restart", "intent-workspace-watch.service"], check=False)
            print(json.dumps(config, indent=2))
        elif args.command == "purge-detailed":
            print(json.dumps(post_json("http://127.0.0.1:9477/v1/detailed-capture/purge"), indent=2))
        elif args.command == "uninstall":
            return command_uninstall(
                keep_data=args.keep_data,
                skip_package=args.skip_package,
                legacy=args.legacy,
            )
    except (OSError, RuntimeError, subprocess.CalledProcessError) as exc:
        print(f"Intent: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
