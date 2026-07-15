from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from tools import workspaces


class WorkspaceConfigTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.home = Path(self.temporary.name) / "home"
        self.home.mkdir()
        self.project = self.home / "project"
        self.project.mkdir()
        self.config_path = self.home / ".config" / "intent-os" / "config.json"

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_add_save_load_and_remove_workspace(self) -> None:
        config = workspaces.add(self.project, {"workspaces": []}, self.home)
        workspaces.save(config, self.config_path)
        self.assertEqual(workspaces.load(self.config_path), {"workspaces": [str(self.project)]})
        self.assertEqual(workspaces.remove(self.project, config), {"workspaces": []})

    def test_rejects_home_and_outside_directories(self) -> None:
        with self.assertRaises(ValueError):
            workspaces.validate_workspace(self.home, self.home)
        with self.assertRaises(ValueError):
            workspaces.validate_workspace(Path(self.temporary.name), self.home)
