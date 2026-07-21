from __future__ import annotations

import io
import json
import sqlite3
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import MagicMock, patch

import tools.intent_osctl as intent_osctl
from tools.intent_osctl import MARKER_END, MARKER_START, SHELL_SOURCES, remove_shell_block, update_shell_integration


class IntentOsCtlTests(unittest.TestCase):
    def test_install_systemd_units_includes_unified_backend_units(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary)
            with patch("tools.intent_osctl.Path.home", return_value=home):
                intent_osctl.install_systemd_units()
            installed = home / ".config" / "systemd" / "user"
            self.assertTrue((installed / "intent-os-backend.target").is_file())
            self.assertIn("intent_engine.scheduled_ingest", (installed / "intent-os-pipeline.service").read_text(encoding="utf-8"))
            self.assertIn("OnCalendar=*-*-* 00/3:00:00", (installed / "intent-os-pipeline.timer").read_text(encoding="utf-8"))

    def test_backend_lifecycle_operates_on_the_single_target(self) -> None:
        with (
            patch("tools.intent_osctl.import_graphical_environment"),
            patch("tools.intent_osctl.install_systemd_units"),
            patch("tools.intent_osctl.install_autostart"),
            patch("tools.intent_osctl.run_systemctl") as systemctl,
        ):
            intent_osctl.enable_services()
            intent_osctl.disable_services()
            intent_osctl.session_start()
        calls = [call.args for call in systemctl.call_args_list]
        self.assertIn(("disable", *intent_osctl.BACKEND_COMPONENT_UNITS), calls)
        self.assertIn(("enable", "--now", "intent-os-backend.target"), calls)
        self.assertIn(("disable", "--now", "intent-os-backend.target"), calls)
        self.assertIn(("start", "intent-os-backend.target"), calls)

    def test_status_includes_safe_scheduler_metadata_and_systemd_details(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            database = Path(temporary) / "intents.db"
            connection = sqlite3.connect(database)
            connection.execute("CREATE TABLE store_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            connection.executemany(
                "INSERT INTO store_metadata(key, value) VALUES (?, ?)",
                [
                    ("scheduled_ingest_last_completed_date", "2026-07-14"),
                    ("scheduled_ingest_last_outcome", "pipeline_error"),
                    ("unrelated", "private exception text"),
                ],
            )
            connection.commit()
            connection.close()
            output = io.StringIO()
            with (
                patch.dict("tools.intent_osctl.os.environ", {"ROLE_B_DB_PATH": str(database)}, clear=False),
                patch("tools.intent_osctl.fetch_json", return_value={"ok": True}),
                patch("tools.intent_osctl.systemd_unit_state", return_value="active"),
                patch("tools.intent_osctl.systemd_timer_next_activation", return_value="Tue 2026-07-21 03:00:00 IST"),
                redirect_stdout(output),
            ):
                self.assertEqual(intent_osctl.command_status(), 0)
        payload = json.loads(output.getvalue())
        self.assertEqual(payload["event_server"], {"ok": True})
        self.assertEqual(payload["pipeline"]["last_completed_date"], "2026-07-14")
        self.assertEqual(payload["pipeline"]["last_outcome"], "pipeline_error")
        self.assertNotIn("private exception text", output.getvalue())

    def test_shell_integration_is_reversible_without_touching_user_content(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary)
            rc = home / ".bashrc"
            rc.write_text("export PATH=/usr/local/bin:$PATH\n", encoding="utf-8")
            update_shell_integration("bash", True, home)
            enabled = rc.read_text(encoding="utf-8")
            self.assertIn(MARKER_START, enabled)
            self.assertIn(f"source {SHELL_SOURCES['bash']}", enabled)

            update_shell_integration("bash", False, home)
            self.assertEqual(rc.read_text(encoding="utf-8"), "export PATH=/usr/local/bin:$PATH\n")

    def test_unfinished_marker_is_not_destroyed(self) -> None:
        contents = f"before\n{MARKER_START}\ncustom\n"
        self.assertEqual(remove_shell_block(contents), contents)

    def test_editor_install_forces_the_bundled_vsix_update(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            vsix = Path(temporary) / "intent-os-vscode.vsix"
            vsix.write_bytes(b"fixture")
            with (
                patch.object(intent_osctl, "VSIX_PATH", vsix),
                patch("tools.intent_osctl.shutil.which", return_value="/usr/bin/cursor"),
                patch("tools.intent_osctl.subprocess.run") as run,
            ):
                intent_osctl.install_editor_extension("cursor", "Cursor")
        run.assert_called_once_with(
            ["/usr/bin/cursor", "--install-extension", str(vsix), "--force"], check=True
        )

    def test_notification_opens_only_configured_preview_launcher(self) -> None:
        intent = {"id": "atlas/one", "label": "Atlas Work", "summary": "Continue the API work.", "tags": ["project:atlas"]}
        notify_result = MagicMock(returncode=0, stdout="default\n")
        with (
            patch("tools.intent_osctl.wait_for_json", side_effect=[{"ok": True}, [intent]]),
            patch("tools.intent_osctl.shutil.which", side_effect=lambda command: "/usr/bin/notify-send" if command == "notify-send" else "/opt/intent-os/preview"),
            patch("tools.intent_osctl.subprocess.run", return_value=notify_result) as run,
            patch("tools.intent_osctl.subprocess.Popen") as popen,
            patch.dict("tools.intent_osctl.os.environ", {"INTENT_OS_PREVIEW_COMMAND": "/opt/intent-os/preview --focus"}, clear=False),
        ):
            self.assertEqual(intent_osctl.command_notify_yesterday(), 0)

        self.assertEqual(run.call_args.args[0][-2:], ["Continue Atlas?", "Continue the API work."])
        launched = popen.call_args.args[0]
        self.assertEqual(launched[:2], ["/opt/intent-os/preview", "--focus"])
        self.assertEqual(launched[-1], "http://127.0.0.1:9479/preview?intent_id=atlas%2Fone&restore_scope=same_project")
        self.assertNotIn("xdg-open", launched)
        self.assertFalse(any("/v1/restore" in str(value) for value in [run.call_args, popen.call_args]))

    def test_notification_without_preview_launcher_never_launches_an_app(self) -> None:
        intent = {"id": "intent-1", "label": "Write docs", "summary": "Review docs."}
        notify_result = MagicMock(returncode=0, stdout="default\n")
        with (
            patch("tools.intent_osctl.wait_for_json", side_effect=[{"ok": True}, [intent]]),
            patch("tools.intent_osctl.shutil.which", return_value="/usr/bin/notify-send"),
            patch("tools.intent_osctl.subprocess.run", return_value=notify_result),
            patch("tools.intent_osctl.subprocess.Popen") as popen,
            patch.dict("tools.intent_osctl.os.environ", {}, clear=True),
        ):
            self.assertEqual(intent_osctl.command_notify_yesterday(), 0)
        popen.assert_not_called()
