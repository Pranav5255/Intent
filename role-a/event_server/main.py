"""FastAPI entry point for the local Intent OS Role A service."""

from __future__ import annotations

import os
from pathlib import Path

from fastapi import Depends, FastAPI, HTTPException, Query, Request, Response, status
from fastapi.middleware.cors import CORSMiddleware

from .detailed_capture import editor_event_is_approved, is_detailed_event, is_enabled, load as load_detailed_config, public_config
from .models import CapturePause, DayExport, EventIn, EventOut, IngestResult
from .redaction import redact_event
from .restore import RestoreResult, ResumePayload, restore
from .storage import EventStore, default_database_path


def get_store(request: Request) -> EventStore:
    return request.app.state.store


def get_detailed_config(request: Request) -> dict[str, object]:
    try:
        return load_detailed_config(request.app.state.detailed_capture_config_path)
    except ValueError as exc:
        raise HTTPException(status_code=500, detail=str(exc)) from exc


def create_app(database_path: str | None = None, detailed_capture_config_path: str | None = None) -> FastAPI:
    app = FastAPI(title="Intent OS Event Server", version="0.1.0")
    app.state.detailed_capture_config_path = (
        Path(detailed_capture_config_path) if detailed_capture_config_path else None
    )

    @app.on_event("startup")
    def initialise_store() -> None:
        app.state.store = EventStore(default_database_path() if database_path is None else Path(database_path))
        app.state.capture_paused = False

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
        store: EventStore = Depends(get_store),
        detailed_config: dict[str, object] = Depends(get_detailed_config),
    ) -> IngestResult | Response:
        if app.state.capture_paused:
            return Response(status_code=status.HTTP_204_NO_CONTENT)
        if is_detailed_event(event.source, event.type) and not is_enabled(event.source, event.type, detailed_config):
            return Response(status_code=status.HTTP_204_NO_CONTENT)
        if event.source == "vscode" and event.type == "document_change" and not editor_event_is_approved(event.payload, detailed_config):
            return Response(status_code=status.HTTP_204_NO_CONTENT)
        try:
            event = redact_event(event)
        except ValueError as exc:
            raise HTTPException(status_code=422, detail=str(exc)) from exc
        inserted, persisted = store.insert(event)
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

    @app.post("/v1/detailed-capture/purge")
    def purge_detailed_capture(store: EventStore = Depends(get_store)) -> dict[str, object]:
        return {"ok": True, "deleted": store.purge_detailed_events()}

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
        return {
            "ok": True,
            "session_type": os.environ.get("XDG_SESSION_TYPE", "unknown"),
            "capture_paused": app.state.capture_paused,
            "sources": store.source_status(),
            "detailed_capture": {**public_config(detailed_config), "event_counts": store.detailed_event_counts()},
        }

    # Compatibility routes preserve the API promised in the original execution plan.
    app.add_api_route("/event", ingest_event, methods=["POST"], response_model=IngestResult, status_code=status.HTTP_201_CREATED)
    app.add_api_route("/events", events, methods=["GET"], response_model=list[EventOut])
    app.add_api_route("/export/day", export_day, methods=["GET"], response_model=DayExport)
    app.add_api_route("/status", source_status, methods=["GET"])
    app.add_api_route("/restore", restore_state, methods=["POST"], response_model=RestoreResult)
    return app


app = create_app()
