from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from event_server.restore import ResumePayload, restore


class RestoreTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp_dir = tempfile.TemporaryDirectory()
        self.root = Path(self.temp_dir.name)
        self.file = self.root / "iam.tf"
        self.file.write_text("resource {}", encoding="utf-8")

    def tearDown(self) -> None:
        self.temp_dir.cleanup()

    @patch("event_server.restore._launch")
    @patch("event_server.restore.shutil.which")
    def test_launches_known_apps_without_a_shell(self, which, launch) -> None:
        which.side_effect = lambda name: {"code": "/usr/bin/code", "firefox": "/usr/bin/firefox", "gnome-terminal": "/usr/bin/gnome-terminal"}.get(name)
        result = restore(
            ResumePayload(
                files=[str(self.file)],
                urls=["https://example.com/docs", "https://example.com/next"],
                shell={"cwd": str(self.root), "last_cmd": "terraform apply"},
            )
        )

        self.assertTrue(result.ok)
        self.assertEqual(result.restored, {"files": 1, "urls": 2, "shell": True})
        self.assertEqual(launch.call_count, 4)
        self.assertEqual(launch.call_args_list[0].args[0], ["/usr/bin/code", "--reuse-window", str(self.file.resolve())])
        self.assertEqual(launch.call_args_list[3].args[0], ["/usr/bin/gnome-terminal", f"--working-directory={self.root.resolve()}"])

    def test_rejects_unsafe_url_scheme(self) -> None:
        with self.assertRaises(ValueError):
            ResumePayload(urls=["file:///home/pranav/private.txt"])
