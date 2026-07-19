from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

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
