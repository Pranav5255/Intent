from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from collectors.workspace.watcher import make_event, should_capture


class WorkspaceWatcherTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name) / "workspace"
        self.root.mkdir()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_captures_small_regular_files_inside_workspace(self) -> None:
        path = self.root / "main.py"
        path.write_text("print('hello')", encoding="utf-8")
        self.assertTrue(should_capture(path, self.root))

    def test_ignores_git_and_paths_outside_workspace(self) -> None:
        git_file = self.root / ".git" / "config"
        git_file.parent.mkdir()
        git_file.write_text("[core]", encoding="utf-8")
        outside = Path(self.temporary.name) / "outside.txt"
        outside.write_text("no", encoding="utf-8")
        self.assertFalse(should_capture(git_file, self.root))
        self.assertFalse(should_capture(outside, self.root))

    def test_ignores_virtual_system_paths(self) -> None:
        self.assertFalse(should_capture(Path("/proc/cpuinfo"), Path("/")))

    def test_emits_fallback_schema(self) -> None:
        event = make_event("file_modify", {"path": "/tmp/a.py", "workspace": "/tmp"})
        self.assertEqual(event["source"], "filesystem")
        self.assertEqual(event["type"], "file_modify")
