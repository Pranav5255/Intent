from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from tools.workspaces import add, config_path, load, remove, save


class WorkspacesTests(unittest.TestCase):
    def test_add_remove_and_persist(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "workspaces.json"
            first = add("~/projects/taskflow-app", {"workspaces": []})
            save(first, path)
            loaded = load(path)
            self.assertEqual(len(loaded["workspaces"]), 1)
            self.assertTrue(loaded["workspaces"][0].endswith("projects/taskflow-app"))

            updated = remove("~/projects/taskflow-app", loaded)
            save(updated, path)
            self.assertEqual(load(path)["workspaces"], [])

    def test_load_missing_returns_empty(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(load(Path(directory) / "missing.json"), {"workspaces": []})

    def test_config_path_uses_xdg(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            resolved = config_path(home)
            self.assertEqual(resolved, home / ".config" / "intent-os" / "workspaces.json")

    def test_invalid_config_raises(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "workspaces.json"
            path.write_text(json.dumps({"workspaces": "bad"}), encoding="utf-8")
            with self.assertRaises(ValueError):
                load(path)


if __name__ == "__main__":
    unittest.main()
