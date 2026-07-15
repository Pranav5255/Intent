from __future__ import annotations

import unittest

from intent_os_shell_send import event_from_nul, safe_command


class ShellSenderTests(unittest.TestCase):
    def test_redacts_sensitive_and_long_commands(self) -> None:
        self.assertEqual(safe_command("terraform apply"), "terraform apply")
        self.assertEqual(safe_command("echo $API_KEY"), "<redacted>")
        self.assertEqual(safe_command("x" * 501), "<redacted>")

    def test_converts_nul_fields_to_canonical_event(self) -> None:
        event = event_from_nul(b"\0".join([b"terraform plan", b"/home/pranav/work/infra", b"0", b"1280", b""]))
        self.assertEqual(event["source"], "shell")
        self.assertEqual(event["type"], "command")
        self.assertEqual(
            event["payload"],
            {"cmd": "terraform plan", "cwd": "/home/pranav/work/infra", "exit_code": 0, "duration_ms": 1280},
        )
