"""FastAPI surface for Role B's stored intent timeline."""

from __future__ import annotations

from datetime import date as calendar_date, timedelta
import os
import time
from typing import Annotated

from fastapi import FastAPI, HTTPException, Query
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse

from intent_engine.context import build_intent_context
from intent_engine.copilot import IntentCopilot
from intent_engine.digest import build_digest
from intent_engine.llm import LLMError
from intent_engine.pipeline import PIPELINE_VERSION, run_pipeline
from intent_engine.current import CurrentIntentEngine
from intent_engine.normalize import normalize_events
from intent_engine.prediction import PredictionEngine
from intent_engine.llm_settings import save_settings, settings_summary
from intent_engine.providers import copilot_enabled, create_copilot_llm, create_label_provider
from intent_engine.resume_select import select_resume_preview
from intent_engine.schemas import CopilotNotConfigured, CopilotQueryRequest, DayExport, LLMSettingsUpdate, PipelineResult, ResumeSelectRequest
from intent_engine.source import RoleAClient, RoleAUnavailableError
from intent_engine.store import IntentStore
from intent_engine.tools import ToolContext, ToolRegistry

API_VERSION = "0.1.0"


def create_app(store: IntentStore | None = None, role_a_client: RoleAClient | None = None) -> FastAPI:
    """Build an injectable application instance for production and tests."""
    application = FastAPI(title="Intent - Role B", version=API_VERSION)
    application.add_middleware(
        CORSMiddleware,
        allow_origins=[
            "http://localhost:3000",
            "http://127.0.0.1:3000",
            "http://localhost:5000",
            "http://localhost:5173",
            "http://127.0.0.1:5173",
            "http://localhost:9479",
            "http://127.0.0.1:9479",
        ],
        allow_credentials=True,
        allow_methods=["GET", "POST", "OPTIONS"],
        allow_headers=["*"],
    )
    database_path = os.environ.get("ROLE_B_DB_PATH", "intents.db")
    application.state.store = store or IntentStore(database_path)
    role_a_base_url = os.environ.get("INTENT_OS_ROLE_A_URL", "").strip() or "http://127.0.0.1:9477"
    application.state.role_a_client = role_a_client or RoleAClient(role_a_base_url)
    application.state.label_provider = create_label_provider()
    application.state.copilot_llm = create_copilot_llm()
    application.state.copilot_enabled = copilot_enabled()
    application.state.current_engine = CurrentIntentEngine(application.state.role_a_client)
    application.state.tools = ToolRegistry(ToolContext(application.state.store, application.state.current_engine))
    application.state.copilot = (
        IntentCopilot(application.state.copilot_llm, application.state.tools)
        if application.state.copilot_llm is not None
        else None
    )
    application.state.prediction_engine = PredictionEngine(application.state.store)

    @application.get("/healthz")
    async def healthz() -> dict:
        return {"ok": True, "version": API_VERSION, "pipeline_version": PIPELINE_VERSION}

    @application.get("/settings/llm")
    async def llm_settings() -> dict:
        return settings_summary()

    @application.put("/settings/llm")
    async def update_llm_settings(request: LLMSettingsUpdate) -> dict:
        try:
            summary = save_settings(request.model_dump())
            application.state.label_provider = create_label_provider()
            application.state.copilot_llm = create_copilot_llm()
            application.state.copilot_enabled = copilot_enabled()
            application.state.copilot = (
                IntentCopilot(application.state.copilot_llm, application.state.tools)
                if application.state.copilot_llm is not None
                else None
            )
            return summary
        except ValueError as exc:
            raise HTTPException(status_code=400, detail="Invalid local provider setting") from exc

    @application.get("/intents/yesterday")
    async def intents_yesterday() -> list:
        return await application.state.store.get_intents_by_date((calendar_date.today() - timedelta(days=1)).isoformat())

    @application.get("/intents")
    async def intents_for_date(date: Annotated[str, Query(...)]) -> list:
        return await application.state.store.get_intents_by_date(_validated_date(date))

    @application.get("/intents/search")
    async def search_intents(
        q: Annotated[str, Query(...)],
        limit: int = 10,
        date_from: str | None = None,
        date_to: str | None = None,
    ) -> list[dict]:
        if not q.strip() or not 1 <= limit <= 100:
            raise HTTPException(status_code=400, detail="q must be non-empty and limit must be between 1 and 100")
        try:
            return await application.state.store.search_intents(q, limit, date_from=date_from, date_to=date_to)
        except ValueError as exc:
            raise HTTPException(status_code=400, detail=str(exc)) from exc

    @application.get("/intents/stats")
    async def intent_stats(
        date_from: Annotated[str, Query(...)],
        date_to: Annotated[str, Query(...)],
        project: str | None = None,
    ) -> dict:
        try:
            return await application.state.store.get_intent_stats(date_from, date_to, project)
        except ValueError as exc:
            raise HTTPException(status_code=400, detail=str(exc)) from exc

    @application.delete("/v1/memory/date/{date}")
    async def forget_date(date: str) -> dict:
        return await application.state.store.delete_date(_validated_date(date))

    @application.delete("/v1/memory/project/{project}")
    async def forget_project(project: str) -> dict:
        if not project.strip():
            raise HTTPException(status_code=400, detail="project must be non-empty")
        return await application.state.store.delete_project(project)

    @application.get("/intents/current")
    async def current_intent():
        return await application.state.current_engine.get_current()

    @application.post("/copilot/query")
    async def copilot_query(request: CopilotQueryRequest):
        if request.mode == "narrative":
            _validate_narrative_dates(request.date_from, request.date_to)
        if not copilot_enabled() or application.state.copilot is None:
            payload = CopilotNotConfigured().model_dump(mode="json")
            return JSONResponse(status_code=503, content=payload)
        try:
            return await application.state.copilot.query(request)
        except LLMError as exc:
            raise HTTPException(status_code=502, detail="Copilot provider request failed") from exc

    @application.get("/copilot/briefing/{intent_id}")
    async def copilot_briefing(intent_id: str):
        if not copilot_enabled() or application.state.copilot is None:
            payload = CopilotNotConfigured().model_dump(mode="json")
            return JSONResponse(status_code=503, content=payload)
        request = CopilotQueryRequest(
            mode="briefing",
            intent_id=intent_id,
            question="Summarize this intent for resume",
        )
        try:
            return await application.state.copilot.query(request)
        except LLMError as exc:
            raise HTTPException(status_code=502, detail="Copilot provider request failed") from exc

    @application.get("/intents/digest")
    async def intents_digest(date: str | None = None) -> dict:
        target_date = _validated_date(date) if date else (calendar_date.today() - timedelta(days=1)).isoformat()
        intents = await application.state.store.get_intents_by_date(target_date)
        return build_digest(intents, target_date)

    @application.get("/intents/prediction")
    async def prediction():
        if os.environ.get("ENABLE_PREDICTION", "false").lower() != "true":
            return None
        yesterday = (calendar_date.today() - timedelta(days=1)).isoformat()
        try:
            await application.state.prediction_engine.train_on_date(yesterday)
            recent = await application.state.role_a_client.fetch_events_since(int(time.time()) - 30 * 60)
            normalized, _warnings = normalize_events(recent)
            return await application.state.prediction_engine.predict(normalized)
        except RoleAUnavailableError:
            return None

    @application.get("/intents/{intent_id}/context")
    async def intent_context(intent_id: str):
        intent = await application.state.store.get_intent_by_id(intent_id)
        if intent is None:
            raise HTTPException(status_code=404, detail="Intent not found")
        return {"intent_id": intent_id, "markdown": build_intent_context(intent)}

    @application.get("/intents/{intent_id}")
    async def intent_by_id(intent_id: str):
        intent = await application.state.store.get_intent_by_id(intent_id)
        if intent is None:
            raise HTTPException(status_code=404, detail="Intent not found")
        return intent

    @application.post("/resume/select")
    async def resume_select(request: ResumeSelectRequest):
        response = await select_resume_preview(application.state.store, request)
        if response is None:
            raise HTTPException(status_code=404, detail="No stored intent matched the selection")
        return response

    @application.post("/pipeline/run")
    async def pipeline_run(date: Annotated[str, Query(...)]) -> PipelineResult:
        date = _validated_date(date)
        try:
            export = await application.state.role_a_client.fetch_export(date)
        except RoleAUnavailableError as exc:
            raise HTTPException(status_code=503, detail="Role A is unavailable") from exc
        return await run_pipeline(export, application.state.store, application.state.label_provider)

    @application.post("/pipeline/run-replay")
    async def pipeline_replay(export: DayExport) -> PipelineResult:
        return await run_pipeline(export, application.state.store, application.state.label_provider)

    @application.post("/pipeline/recompute")
    async def pipeline_recompute(date: Annotated[str, Query(...)]) -> PipelineResult:
        date = _validated_date(date)
        try:
            export = await application.state.role_a_client.fetch_export(date)
        except RoleAUnavailableError as exc:
            raise HTTPException(status_code=503, detail="Role A is unavailable") from exc
        await application.state.store.delete_date(date)
        return await run_pipeline(export, application.state.store, application.state.label_provider, force=True)

    return application


def _validated_date(value: str) -> str:
    if len(value) != 10:
        return _invalid_date()
    try:
        return calendar_date.fromisoformat(value).isoformat()
    except ValueError:
        return _invalid_date()


def _invalid_date() -> str:
    raise HTTPException(status_code=400, detail="date must be a real YYYY-MM-DD calendar date")


def _validate_narrative_dates(date_from: str | None, date_to: str | None) -> None:
    if not date_from or not date_to:
        raise HTTPException(status_code=400, detail="narrative mode requires date_from and date_to")
    start = _validated_date(date_from)
    end = _validated_date(date_to)
    if start > end:
        raise HTTPException(status_code=400, detail="date_from must not be later than date_to")


app = create_app()
