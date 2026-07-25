"""Privacy-preserving activity timing signals.

No key values, mouse coordinates, or window content are retained: only event
times and coarse kinds are used to tune polling and expose capture health.
"""

from __future__ import annotations

import time
from collections import deque
from threading import Lock
from typing import Callable


class ActivityFeed:
    def __init__(self, clock: Callable[[], float] = time.monotonic) -> None:
        self._clock = clock
        self._lock = Lock()
        self._last_activity_at = clock()
        self._last_key_at: float | None = None
        self._key_times: deque[float] = deque()
        self._reported_idle_ms: int | None = None

    def record(self, kind: str) -> None:
        if kind not in {"key", "mouse", "shell", "focus"}:
            raise ValueError("activity kind must be key, mouse, shell, or focus")
        now = self._clock()
        with self._lock:
            self._last_activity_at = now
            self._reported_idle_ms = None
            if kind == "key":
                self._last_key_at = now
                self._key_times.append(now)
                self._prune_keys(now)

    def set_idle_ms(self, idle_ms: int) -> None:
        if idle_ms < 0:
            raise ValueError("idle_ms must be non-negative")
        with self._lock:
            self._reported_idle_ms = idle_ms

    def _prune_keys(self, now: float) -> None:
        while self._key_times and now - self._key_times[0] > 0.5:
            self._key_times.popleft()

    def idle_ms(self) -> int:
        now = self._clock()
        with self._lock:
            if self._reported_idle_ms is not None:
                return self._reported_idle_ms
            return max(0, int((now - self._last_activity_at) * 1000))

    def is_typing(self) -> bool:
        now = self._clock()
        with self._lock:
            return self._last_key_at is not None and now - self._last_key_at < 0.3

    def is_keyboard_burst(self) -> bool:
        now = self._clock()
        with self._lock:
            self._prune_keys(now)
            return len(self._key_times) >= 3

    def is_active(self, threshold_ms: int) -> bool:
        return self.idle_ms() < threshold_ms

    def recommended_poll_interval_sec(self) -> float:
        idle = self.idle_ms()
        if idle < 5_000:
            return 2.0
        if idle < 60_000:
            return 5.0
        return 10.0

    def snapshot(self) -> dict[str, object]:
        return {
            "idle_ms": self.idle_ms(),
            "is_typing": self.is_typing(),
            "is_keyboard_burst": self.is_keyboard_burst(),
            "recommended_poll_interval_sec": self.recommended_poll_interval_sec(),
        }
