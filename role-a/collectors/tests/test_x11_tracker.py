from __future__ import annotations

import unittest

from collectors.x11.tracker import ActiveWindow, build_event, get_active_window, parse_wm_class


class X11TrackerTests(unittest.TestCase):
    def test_parse_wm_class_uses_application_class(self) -> None:
        self.assertEqual(parse_wm_class('WM_CLASS(STRING) = "Navigator", "firefox"'), "firefox")
        self.assertEqual(parse_wm_class('WM_CLASS(STRING) = "code", "Code"'), "code")

    def test_active_window_uses_x11_commands(self) -> None:
        responses = {
            ("xdotool", "getactivewindow"): "1234",
            ("xdotool", "getwindowname", "1234"): "iam.tf - Visual Studio Code",
            ("xprop", "-id", "1234", "WM_CLASS"): 'WM_CLASS(STRING) = "code", "Code"',
        }

        window = get_active_window(lambda command: responses[tuple(command)])
        self.assertEqual(window, ActiveWindow("1234", "code", "iam.tf - Visual Studio Code"))

    def test_event_has_canonical_linux_shape(self) -> None:
        event = build_event(ActiveWindow("12", "firefox", "Terraform docs"))
        self.assertEqual(event["source"], "linux")
        self.assertEqual(event["type"], "app_focus")
        self.assertEqual(event["payload"], {"app": "firefox", "title": "Terraform docs", "window_id": "12"})
