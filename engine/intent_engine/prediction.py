"""F10 historical-prefix intent prediction."""

from __future__ import annotations

from intent_engine.schemas import Intent, NormalizedEvent, PredictionResponse
from intent_engine.store import IntentStore


class PredictionEngine:
    """Match a recent event prefix against previously persisted child intents."""

    def __init__(self, store: IntentStore) -> None:
        self.store = store
        self.prefix_index: dict[tuple[str, str, str], list[Intent]] = {}

    async def train_on_date(self, date: str) -> None:
        self.prefix_index.clear()
        roots = await self.store.get_intents_by_date(date)
        for root in roots:
            for intent in _walk_intents(root):
                if intent.prefix is not None:
                    self.prefix_index.setdefault(intent.prefix, []).append(intent)

    async def predict(self, recent_events: list[NormalizedEvent]) -> PredictionResponse | None:
        if len(recent_events) < 3:
            return None
        prefix = _event_prefix(recent_events[-3:])
        matches = self.prefix_index.get(prefix, [])
        if len(matches) < 2:
            return None
        match = max(matches, key=lambda intent: (intent.end_ts, intent.start_ts, intent.id))
        confidence = min(0.95, 0.7 + 0.1 * (len(matches) - 2))
        return PredictionResponse(
            predicted_label=match.label,
            confidence=confidence,
            resume_payload=match.resume_payload,
        )


def _walk_intents(intent: Intent):
    yield intent
    for child in intent.children:
        yield from _walk_intents(child)


def _event_prefix(events: list[NormalizedEvent]) -> tuple[str, str, str]:
    final = events[-1]
    project = final.entities.project_paths[0] if final.entities.project_paths else ""
    return (events[0].family, events[1].category, final.entities.command_family or project)
