from __future__ import annotations

import unittest
from unittest.mock import patch

from collectors.x11.tracker import run_tracker


class X11RuntimeTests(unittest.TestCase):
    @patch("collectors.x11.tracker.shutil.which", return_value=None)
    def test_missing_x11_tools_report_degraded_mode(self, _which) -> None:
        self.assertEqual(run_tracker("http://127.0.0.1:9477/v1/event", 1, once=True), 2)
