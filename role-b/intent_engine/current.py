"""F11: infer the user's current work from the most recent activity window."""

from __future__ import annotations

import time

from intent_engine.cluster import cluster_session
from intent_engine.labeling import build_cluster_hints
from intent_engine.normalize import intelligence_text, normalize_events
from intent_engine.providers import create_label_provider
from intent_engine.schemas import CurrentIntent
from intent_engine.sessionize import sessionize
from intent_engine.source import RoleAClient, RoleAUnavailableError


class CurrentIntentEngine:
    """Infer and briefly cache the most recent sufficiently confident work."""

    def __init__(self, role_a_client: RoleAClient) -> None:
        self.client = role_a_client
        self.cache: dict[int, CurrentIntent] = {}
        self.cached_intent: CurrentIntent | None = None
        self.cache_expires_at: int = 0
        self._label_provider = create_label_provider()

    async def get_current(self) -> CurrentIntent | None:
        now_ts = int(time.time())
        if self.cached_intent is not None and now_ts < self.cache_expires_at:
            return self.cached_intent if self.cached_intent.confidence >= 0.5 else None

        try:
            events = await self.client.fetch_events_since(now_ts - 30 * 60)
        except RoleAUnavailableError:
            return None

        normalized, _warnings = normalize_events(events)
        sessions = await sessionize(normalized)
        clusters: list = []
        for session in sessions:
            clusters.extend(await cluster_session(session))
        if not clusters:
            return None

        cluster = clusters[-1]
        event_count = len(cluster)
        confidence = 0.8 if event_count > 5 else 0.6 if event_count >= 3 else 0.3
        if confidence < 0.5:
            return None

        cluster_text = "\n".join(intelligence_text(event, index) for index, event in enumerate(cluster, start=1))
        hints = build_cluster_hints(cluster)
        label_result = await self._label_provider.label_cluster(cluster_text, hints=hints)
        intent = CurrentIntent(
            label=label_result["label"],
            summary=label_result["summary"],
            confidence=confidence,
            since_ts=cluster[0].ts,
        )
        self.cached_intent = intent
        self.cache[now_ts] = intent
        self.cache_expires_at = now_ts + 60
        return intent
