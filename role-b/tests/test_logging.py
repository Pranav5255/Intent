from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parents[1]))

from intent_engine.logging import DiagnosticsLogger, log_line_safe


class DiagnosticsLoggerTests(unittest.TestCase):
    @staticmethod
    def read_lines(path: Path) -> list[dict]:
        return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines()]

    def test_log_methods_append_expected_records(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "diagnostics.jsonl"
            logger = DiagnosticsLogger(str(path))
            logger.log_event_validation_error("event-1", "payload missing required field")
            logger.log_pipeline_run("2026-07-13", "0123456789abcdefextra", "complete", 26, 42, 1)
            logger.log_cache_hit("2026-07-13", "fedcba9876543210extra")
            records = self.read_lines(path)

        self.assertEqual([record["type"] for record in records], ["event_validation_error", "pipeline_run", "cache_hit"])
        self.assertEqual(records[1]["source_hash"], "0123456789abcdef")
        self.assertEqual(records[2]["source_hash"], "fedcba9876543210")
        self.assertTrue(records[0]["timestamp"].endswith("+00:00"))
        self.assertEqual(logger.buffer, [])

    def test_flush_writes_pending_lines_once(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "diagnostics.jsonl"
            logger = DiagnosticsLogger(str(path))
            logger.buffer.append(log_line_safe({"type": "cache_hit", "date": "2026-07-13"}))
            logger.flush()
            logger.flush()
            records = self.read_lines(path)

        self.assertEqual(records, [{"type": "cache_hit", "date": "2026-07-13"}])
        self.assertEqual(logger.buffer, [])

    def test_log_line_safe_excludes_sensitive_event_data(self) -> None:
        safe = json.loads(log_line_safe({
            "type": "event_validation_error",
            "error": "payload missing required field",
            "payload": {"text": "private document content"},
            "url": "https://example.com/private",
            "raw": {"secret": "value"},
        }))
        secret_error = json.loads(log_line_safe({"type": "event_validation_error", "error": "token=abc"}))
        document_error = json.loads(log_line_safe({
            "type": "event_validation_error", "error": '{"payload":{"text":"private"}}',
        }))

        self.assertEqual(safe, {"type": "event_validation_error", "error": "payload missing required field"})
        self.assertEqual(secret_error["error"], "<redacted>")
        self.assertEqual(document_error["error"], "<redacted>")


if __name__ == "__main__":
    unittest.main()
