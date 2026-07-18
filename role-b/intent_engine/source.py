"""Role A HTTP client and replay-fixture loader for the Role B pipeline."""

from __future__ import annotations

import json
import re
from datetime import date as calendar_date
from pathlib import Path
from typing import Any

import httpx
from pydantic import ValidationError

from intent_engine.schemas import DayExport, RawEvent


_ISO_DATE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
_REQUEST_TIMEOUT_SECONDS = 10.0


class RoleAUnavailableError(RuntimeError):
    """Raised when Role A cannot be reached or is temporarily unavailable."""


class RoleAClient:
    """Small async client for the public Role A event-service API."""

    def __init__(self, base_url: str = "http://127.0.0.1:9477") -> None:
        self.base_url = base_url.rstrip("/")

    async def fetch_export(self, date: str) -> DayExport:
        """Fetch and validate Role A's complete export for one ISO calendar day."""

        self._validate_iso_date(date)
        data = await self._get_json("/v1/export/day", params={"date": date})
        try:
            return DayExport.model_validate(data)
        except ValidationError as exc:
            raise ValueError("Role A returned an invalid day export") from exc

    async def fetch_events_since(self, since_ts: int) -> list[RawEvent]:
        """Fetch and validate events Role A observed at or after ``since_ts``."""

        data = await self._get_json("/v1/events", params={"since": since_ts})
        try:
            return [RawEvent.model_validate(event) for event in data]
        except (TypeError, ValidationError) as exc:
            raise ValueError("Role A returned an invalid events response") from exc

    async def health(self) -> dict[str, Any]:
        """Return Role A's health response without changing its public fields."""

        data = await self._get_json("/healthz")
        if not isinstance(data, dict):
            raise ValueError("Role A returned an invalid health response")
        return data

    async def _get_json(self, path: str, *, params: dict[str, Any] | None = None) -> Any:
        try:
            async with httpx.AsyncClient(timeout=_REQUEST_TIMEOUT_SECONDS) as client:
                response = await client.get(f"{self.base_url}{path}", params=params)
        except (httpx.ConnectError, httpx.TimeoutException, httpx.NetworkError) as exc:
            raise RoleAUnavailableError(f"Role A is unavailable: {exc}") from exc

        if response.status_code == httpx.codes.SERVICE_UNAVAILABLE:
            raise RoleAUnavailableError("Role A is unavailable (HTTP 503)")

        response.raise_for_status()
        try:
            return response.json()
        except json.JSONDecodeError as exc:
            raise ValueError("Role A returned invalid JSON") from exc

    @staticmethod
    def _validate_iso_date(value: str) -> None:
        if not _ISO_DATE.fullmatch(value):
            raise ValueError("date must be YYYY-MM-DD")
        try:
            calendar_date.fromisoformat(value)
        except ValueError as exc:
            raise ValueError("date must be YYYY-MM-DD") from exc


def load_replay_fixture(path: str) -> DayExport:
    """Read a Role A day-export JSON fixture and validate its schema."""

    try:
        content = Path(path).read_text(encoding="utf-8")
    except FileNotFoundError:
        raise
    except OSError as exc:
        raise ValueError(f"Could not read replay fixture: {path}") from exc

    try:
        data = json.loads(content)
    except json.JSONDecodeError as exc:
        raise ValueError(f"Replay fixture contains invalid JSON: {path}") from exc

    try:
        return DayExport.model_validate(data)
    except ValidationError as exc:
        raise ValueError(f"Replay fixture does not match DayExport schema: {path}") from exc
