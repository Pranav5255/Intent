from __future__ import annotations

import unittest

from collectors.activity.feed import ActivityFeed


class ActivityFeedTests(unittest.TestCase):
    def setUp(self) -> None:
        self.now = 100.0
        self.feed = ActivityFeed(clock=lambda: self.now)

    def test_keyboard_burst_uses_only_recent_key_timing(self) -> None:
        for _ in range(3):
            self.feed.record("key")
            self.now += 0.1
        self.assertTrue(self.feed.is_typing())
        self.assertTrue(self.feed.is_keyboard_burst())
        self.now += 0.51
        self.assertFalse(self.feed.is_keyboard_burst())
        self.assertFalse(self.feed.is_typing())

    def test_idle_drives_the_recommended_poll_interval(self) -> None:
        self.feed.record("focus")
        self.now += 5.1
        self.assertEqual(self.feed.recommended_poll_interval_sec(), 5.0)
        self.now += 55
        self.assertEqual(self.feed.recommended_poll_interval_sec(), 10.0)
