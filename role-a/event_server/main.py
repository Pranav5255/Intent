"""FastAPI entry point for the local Intent OS Role A service."""

from __future__ import annotations

import os
from logging import Logger
from pathlib import Path

from fastapi import Depends, FastAPI, HTTPException, Query, Request, Response, status
from fastapi.middleware.cors import CORSMiddleware

from .detailed_capture import editor_event_is_approved, is_detailed_event, is_enabled, load as load_detailed_config, public_config
from .ingestion import EventWriter, IngestionUnavailable
from .logging_setup import configure_jsonl_logger
from .models import CapturePause, DayExport, EventIn, EventOut, IngestResult, RetentionPurge
from .redaction import redact_event
from .restore import RestoreResult, ResumePayload, restore
from .storage import EventStore, default_database_path
from .url_filter import is_url_blocked, load as load_blocked_domains, redact_blocked_browser_event
from collectors.activity.feed import ActivityFeed


def get_store(request: Request) -> EventStore:
    return request.app.state.store


def get_writer(request: Request) -> EventWriter:
    return request.app.state.writer


def get_detailed_config(request: Request) -> dict[str, object]:
    try:
        return load_detailed_config(request.app.state.detailed_capture_config_path)
    except ValueError as exc:
        raise HTTPException(status_code=500, detail=str(exc)) from exc


def get_blocked_domains_config(request: Request) -> dict[str, list[str]]:
    try:
        return load_blocked_domains(request.app.state.blocked_domains_config_path)
    except ValueError as exc:
        raise HTTPException(status_code=500, detail=str(exc)) from exc


def _source_staleness_seconds() -> int:
    try:
        value = int(os.environ.get("INTENT_OS_SOURCE_STALE_AFTER_SECONDS", "1800"))
    except ValueError:
        return 1800
    return value if value > 0 else 1800


def _environment_int(name: str, default: int, *, minimum: int = 1) -> int:
    """Read an operational tuning value without making startup fragile."""

    try:
        value = int(os.environ.get(name, str(default)))
    except ValueError:
        return default
    return value if value >= minimum else default


def _writer_settings(
    *,
    queue_capacity: int | None,
    batch_size: int | None,
    batch_wait_ms: int | None,
    submit_timeout_seconds: float | None,
) -> dict[str, int | float]:
    return {
        "queue_capacity": queue_capacity
        if queue_capacity is not None
        else _environment_int("INTENT_OS_INGEST_QUEUE_CAPACITY", 1_024),
        "max_batch_size": batch_size
        if batch_size is not None
        else _environment_int("INTENT_OS_INGEST_BATCH_SIZE", 100),
        "max_batch_wait_ms": batch_wait_ms
        if batch_wait_ms is not None
        else _environment_int("INTENT_OS_INGEST_BATCH_WAIT_MS", 25, minimum=0),
        "submit_timeout_seconds": submit_timeout_seconds
        if submit_timeout_seconds is not None
        else _environment_int("INTENT_OS_INGEST_SUBMIT_TIMEOUT_SECONDS", 5),
    }


def _record_activity(feed: ActivityFeed, event: EventIn) -> None:
    if event.source == "linux" and event.type == "app_focus":
        feed.record("focus")
    elif event.source == "shell" and event.type == "command":
        feed.record("shell")
    elif event.source == "vscode" and event.type == "document_change":
        feed.record("key")


def _logger(app: FastAPI) -> Logger:
    return app.state.logger


def create_app(
    database_path: str | None = None,
    detailed_capture_config_path: str | None = None,
    blocked_domains_config_path: str | None = None,
    *,
    ingestion_queue_capacity: int | None = None,
    ingestion_batch_size: int | None = None,
    ingestion_batch_wait_ms: int | None = None,
    ingestion_submit_timeout_seconds: float | None = None,
) -> FastAPI:
    app = FastAPI(title="Intent OS Event Server", version="0.1.0")
    app.state.detailed_capture_config_path = (
        Path(detailed_capture_config_path) if detailed_capture_config_path else None
    )
    app.state.blocked_domains_config_path = Path(blocked_domains_config_path) if blocked_domains_config_path else None

    @app.on_event("startup")
    def initialise_store() -> None:
        app.state.store = EventStore(default_database_path() if database_path is None else Path(database_path))
        app.state.writer = EventWriter(
            app.state.store,
            **_writer_settings(
                queue_capacity=ingestion_queue_capacity,
                batch_size=ingestion_batch_size,
                batch_wait_ms=ingestion_batch_wait_ms,
                submit_timeout_seconds=ingestion_submit_timeout_seconds,
            ),
        )
        app.state.capture_paused = False
        app.state.activity_feed = ActivityFeed()
        app.state.logger = configure_jsonl_logger("event-server", "event-server.jsonl")

    @app.on_event("shutdown")
    def drain_writer() -> None:
        app.state.writer.close()

    extension_origin = os.environ.get("INTENT_OS_FIREFOX_EXTENSION_ORIGIN")
    app.add_middleware(
        CORSMiddleware,
        allow_origins=[extension_origin] if extension_origin else [],
        allow_methods=["POST", "OPTIONS"],
        allow_headers=["content-type"],
    )

    @app.get("/healthz")
    def healthz(store: EventStore = Depends(get_store)) -> dict[str, object]:
        return {"ok": True, "version": app.version, "database": str(store.database_path)}

    @app.post("/v1/event", response_model=IngestResult, status_code=status.HTTP_201_CREATED)
    def ingest_event(
        event: EventIn,
        response: Response,
        writer: EventWriter = Depends(get_writer),
        detailed_config: dict[str, object] = Depends(get_detailed_config),
        blocked_domains_config: dict[str, list[str]] = Depends(get_blocked_domains_config),
    ) -> IngestResult | Response:
        if app.state.capture_paused:
            writer.record_drop("capture_paused")
            return Response(status_code=status.HTTP_204_NO_CONTENT)
        if is_detailed_event(event.source, event.type) and not is_enabled(event.source, event.type, detailed_config):
            writer.record_drop("detailed_capture_disabled")
            return Response(status_code=status.HTTP_204_NO_CONTENT)
        if event.source == "vscode" and event.type == "document_change" and not editor_event_is_approved(event.payload, detailed_config):
            writer.record_drop("editor_not_approved")
            return Response(status_code=status.HTTP_204_NO_CONTENT)
        try:
            event = redact_event(event)
        except ValueError as exc:
            writer.record_drop("redaction_rejected")
            _logger(app).error("event_rejected", extra={"event": "event_rejected", "error_type": type(exc).__name__})
            raise HTTPException(status_code=422, detail=str(exc)) from exc
        if event.source == "firefox" and event.type in {"tab_change", "user_action"}:
            url = event.payload.get("url")
            if isinstance(url, str) and is_url_blocked(url, blocked_domains_config["blocked_domains"]):
                copier = getattr(event, "model_copy", event.copy)
                event = copier(update={"payload": redact_blocked_browser_event(event.payload)})
        try:
            outcome = writer.submit(event)
        except IngestionUnavailable as exc:
            _logger(app).error(
                "event_persistence_unavailable",
                extra={"event": "event_persistence_unavailable", "source": event.source, "error_type": type(exc).__name__},
            )
            raise HTTPException(status_code=status.HTTP_503_SERVICE_UNAVAILABLE, detail="event persistence is temporarily unavailable") from exc
        inserted, persisted = outcome.inserted, outcome.event
        if inserted:
            _record_activity(app.state.activity_feed, persisted)
            _logger(app).info(
                "event_ingested",
                extra={"event": "event_ingested", "source": persisted.source, "type": persisted.type, "id": str(persisted.id)},
            )
        if not inserted:
            response.status_code = status.HTTP_200_OK
        return IngestResult(inserted=inserted, event=persisted)

    @app.get("/v1/events", response_model=list[EventOut])
    def events(
        store: EventStore = Depends(get_store),
        date: str | None = Query(default=None),
        since: int | None = Query(default=None, ge=0),
    ) -> list[EventOut]:
        try:
            return store.list_events(date_value=date, since=since)
        except ValueError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.get("/v1/export/day", response_model=DayExport)
    def export_day(date: str, store: EventStore = Depends(get_store)) -> DayExport:
        try:
            return store.export_day(date)
        except ValueError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc

    @app.get("/v1/detailed-capture/config")
    def detailed_capture_config(detailed_config: dict[str, object] = Depends(get_detailed_config)) -> dict[str, object]:
        return public_config(detailed_config)

    @app.get("/v1/config")
    def config(blocked_domains_config: dict[str, list[str]] = Depends(get_blocked_domains_config)) -> dict[str, object]:
        return {"blocked_domains": blocked_domains_config["blocked_domains"]}

    @app.post("/v1/detailed-capture/purge")
    def purge_detailed_capture(store: EventStore = Depends(get_store)) -> dict[str, object]:
        return {"ok": True, "deleted": store.purge_detailed_events()}

    @app.get("/v1/retention/preview")
    def retention_preview(
        detailed_days: int | None = Query(default=None, ge=1, le=36_500),
        metadata_days: int | None = Query(default=None, ge=1, le=36_500),
        store: EventStore = Depends(get_store),
    ) -> dict[str, object]:
        if detailed_days is None and metadata_days is None:
            raise HTTPException(status_code=422, detail="at least one retention window must be provided")
        return store.retention_preview(detailed_days=detailed_days, metadata_days=metadata_days)

    @app.post("/v1/retention/purge")
    def purge_retention(payload: RetentionPurge, store: EventStore = Depends(get_store)) -> dict[str, object]:
        if not payload.confirm:
            raise HTTPException(status_code=status.HTTP_409_CONFLICT, detail="set confirm=true after reviewing retention preview")
        return {
            "ok": True,
            **store.purge_retention(detailed_days=payload.detailed_days, metadata_days=payload.metadata_days),
        }

    @app.post("/v1/restore", response_model=RestoreResult)
    def restore_state(payload: ResumePayload) -> RestoreResult:
        return restore(payload)

    @app.post("/v1/capture/pause")
    def set_capture_paused(payload: CapturePause, request: Request) -> dict[str, object]:
        # The tray uses this in the local, single-user graphical session.
        # It intentionally does not alter stored events.
        request.app.state.capture_paused = payload.paused
        return {"ok": True, "paused": request.app.state.capture_paused}

    @app.get("/v1/status")
    def source_status(
        store: EventStore = Depends(get_store), detailed_config: dict[str, object] = Depends(get_detailed_config)
    ) -> dict[str, object]:
        sources = store.source_status(stale_after_seconds=_source_staleness_seconds())
        return {
            "ok": True,
            "session_type": os.environ.get("XDG_SESSION_TYPE", "unknown"),
            "capture_paused": app.state.capture_paused,
            "ingestion": app.state.writer.snapshot(),
            "sources": sources,
            "services": {
                "event_server": True,
                "x11_tracker": bool(sources["linux"]["healthy"]),
                "workspace_watch": bool(sources["filesystem"]["healthy"]),
            },
            "activity": app.state.activity_feed.snapshot(),
            "detailed_capture": {**public_config(detailed_config), "event_counts": store.detailed_event_counts()},
        }

    # Compatibility routes preserve the API promised in the original execution plan.
    app.add_api_route("/event", ingest_event, methods=["POST"], response_model=IngestResult, status_code=status.HTTP_201_CREATED)
    app.add_api_route("/events", events, methods=["GET"], response_model=list[EventOut])
    app.add_api_route("/export/day", export_day, methods=["GET"], response_model=DayExport)
    app.add_api_route("/status", source_status, methods=["GET"])
    app.add_api_route("/restore", restore_state, methods=["POST"], response_model=RestoreResult)
    app.add_api_route("/retention/preview", retention_preview, methods=["GET"])
    app.add_api_route("/retention/purge", purge_retention, methods=["POST"])
    return app


app = create_app()
