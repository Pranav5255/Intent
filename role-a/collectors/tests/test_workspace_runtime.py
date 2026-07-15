from __future__ import annotations

import tempfile
import threading
import unittest
from pathlib import Path
from types import SimpleNamespace

from collectors.workspace.watcher import WorkspaceHandler


class WorkspaceRuntimeTests(unittest.TestCase):
    def test_modified_file_is_debounced_and_emitted(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path = root / "main.py"
            path.write_text("print(1)", encoding="utf-8")
            received: list[dict[str, object]] = []
            emitted = threading.Event()

            def capture(event: dict[str, object]) -> None:
                received.append(event)
                emitted.set()

            handler = WorkspaceHandler(root, capture, debounce_seconds=0.01)
            handler.on_modified(SimpleNamespace(src_path=str(path), is_directory=False))
            self.assertTrue(emitted.wait(1))
            self.assertEqual(received[0]["type"], "file_modify")
            self.assertEqual(received[0]["payload"]["path"], str(path))
