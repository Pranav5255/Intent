from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

import tools.intent_osctl as intent_osctl
from tools.intent_osctl import MARKER_END, MARKER_START, SHELL_SOURCES, remove_shell_block, update_shell_integration


class IntentOsCtlTests(unittest.TestCase):
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
