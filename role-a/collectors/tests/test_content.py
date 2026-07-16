from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from collectors.content import MAX_EXCERPT_CHARS, excluded, extract


class ContentExtractionTests(unittest.TestCase):
    def test_extracts_a_bounded_text_excerpt(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "notes.md"
            path.write_text("word " * 2_000, encoding="utf-8")
            result = extract(path)
            self.assertEqual(result["kind"], "text")
            self.assertLessEqual(len(result["excerpt"]), MAX_EXCERPT_CHARS)
            self.assertEqual(result["path"], str(path.resolve()))

    def test_excludes_credential_like_paths(self) -> None:
        self.assertTrue(excluded(Path("/home/test/.ssh/id_ed25519")))
        self.assertTrue(excluded(Path("/proc/cpuinfo")))
