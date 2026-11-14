//! Intent routes: timeline, search, digest, context, resume, pipeline, memory.

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use intent_engine::TemplateLabelProvider;
use intent_engine::context::build_intent_context;
use intent_engine::digest::build_digest;
use intent_engine::normalize::normalize_events;
use intent_engine::prediction::IndexKey;
use intent_engine::resume_select::{RerankOptions, Reranker, select_resume_preview};
use intent_store::now_ts;
use intent_types::{CopilotMode, CopilotQueryRequest, DayExport, Intent, ResumeSelectRequest};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{ApiError, ApiResult, JsonBody, QueryParams};
use crate::llm::{current_intent, hybrid_search};
use crate::pipeline::{self, RunMode};
use crate::state::AppState;
use crate::util;

const BAD: StatusCode = StatusCode::BAD_REQUEST;

fn check_date(value: &str) -> ApiResult<()> {
    if util::is_valid_date(value) {
        Ok(())
    } else {
        Err(ApiError::bad_request("date must be a real YYYY-MM-DD calendar date"))
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> ApiResult<T> {
    Ok(tokio::task::spawn_blocking(f).await?)
}

async fn intents_for(state: &AppState, date: String) -> ApiResult<Vec<Intent>> {
    let store = state.store.clone();
    blocking(move || store.intents_for_date(&date)).await?.map_err(|e| ApiError::store(e, BAD))
}

pub async fn yesterday(State(state): State<AppState>) -> ApiResult<Json<Vec<Intent>>> {
    let date = util::yesterday(&state.store);
    Ok(Json(intents_for(&state, date).await?))
}

#[derive(Deserialize)]
pub struct OptDate {
    date: Option<String>,
}

pub async fn by_date(State(state): State<AppState>, QueryParams(q): QueryParams<OptDate>) -> ApiResult<Json<Vec<Intent>>> {
    let date = q.date.ok_or_else(|| ApiError::unprocessable("date is required"))?;
    check_date(&date)?;
    Ok(Json(intents_for(&state, date).await?))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    q: String,
    limit: Option<i64>,
    date_from: Option<String>,
    date_to: Option<String>,
    /// Restrict hits to intents tagged `project:<name>` (the overlay's scope chip).
    project: Option<String>,
}

pub async fn search(State(state): State<AppState>, QueryParams(q): QueryParams<SearchQuery>) -> ApiResult<Response> {
    let limit = q.limit.unwrap_or(10);
    if q.q.trim().is_empty() || !(1..=100).contains(&limit) {
        return Err(ApiError::bad_request("q must be non-empty and limit must be between 1 and 100"));
    }
    let embedder = state.llm().providers.embedder.clone();
    let project = q.project.filter(|p| !p.trim().is_empty());
    // Over-fetch when scoped so filtering by project still fills the page.
    let fetch = if project.is_some() { (limit * 4).min(100) } else { limit };
    let hits = hybrid_search(&state.store, embedder.as_ref(), &q.q, fetch as usize, q.date_from, q.date_to)
        .await
        .map_err(|e| ApiError::store(e, BAD))?;
    let hits = match project {
        None => hits,
        Some(project) => {
            let store = state.store.clone();
            blocking(move || {
                hits.into_iter()
                    .filter(|h| {
                        store.intent_by_id(&h.id).ok().flatten().is_some_and(|i| i.project_tag() == Some(project.as_str()))
                    })
                    .take(limit as usize)
                    .collect::<Vec<_>>()
            })
            .await?
        }
    };
    Ok(Json(hits).into_response())
}

#[derive(Deserialize)]
pub struct StatsQuery {
    date_from: String,
    date_to: String,
    project: Option<String>,
}

pub async fn stats(State(state): State<AppState>, QueryParams(q): QueryParams<StatsQuery>) -> ApiResult<Response> {
    let store = state.store.clone();
    let report = blocking(move || store.stats(&q.date_from, &q.date_to, q.project.as_deref()))
        .await?
        .map_err(|e| ApiError::store(e, BAD))?;
    Ok(Json(report).into_response())
}

pub async fn digest(State(state): State<AppState>, QueryParams(q): QueryParams<OptDate>) -> ApiResult<Response> {
    let date = match q.date {
        Some(d) => {
            check_date(&d)?;
            d
        }
        None => util::yesterday(&state.store),
    };
    let intents = intents_for(&state, date.clone()).await?;
    Ok(Json(build_digest(&intents, &date)).into_response())
}

pub async fn current(State(state): State<AppState>) -> Response {
    let runtime = state.llm();
    let usable = state.inference_usable().await;
    let result = match (&runtime.providers.label, usable) {
        (Some(label), true) => current_intent(&state.store, &state.current, label.as_ref()).await,
        _ => current_intent(&state.store, &state.current, &TemplateLabelProvider).await,
    };
    Json(result).into_response()
}

pub async fn prediction(State(state): State<AppState>) -> ApiResult<Response> {
    if !state.enable_prediction {
        return Ok(Json(Value::Null).into_response());
    }
    let store = state.store.clone();
    let yesterday = util::yesterday(&state.store);
    let (revision, roots, recent) = blocking(move || -> intent_store::Result<_> {
        let meta = store.day_meta(&yesterday)?;
        let roots = store.intents_for_date(&yesterday)?;
        let recent = store.events_since(now_ts() - 30 * 60)?;
        Ok((meta.revision, roots, recent))
    })
    .await?
    .map_err(|e| ApiError::store(e, BAD))?;
    let (normalized, _) = normalize_events(&recent);
    let key = IndexKey { date: util::yesterday(&state.store), revision: revision * 1_000_003 + roots.len() as i64 };
    let predicted = state.prediction.predict(key, move || roots, &normalized);
    Ok(Json(predicted).into_response())
}

pub async fn by_id(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Intent>> {
    let store = state.store.clone();
    blocking(move || store.intent_by_id(&id))
        .await?
        .map_err(|e| ApiError::store(e, BAD))?
        .map(Json)
        .ok_or_else(|| ApiError::not_found("Intent not found"))
}

pub async fn context(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let store = state.store.clone();
    let lookup = id.clone();
    let intent = blocking(move || store.intent_by_id(&lookup))
        .await?
        .map_err(|e| ApiError::store(e, BAD))?
        .ok_or_else(|| ApiError::not_found("Intent not found"))?;
    Ok(Json(json!({"intent_id": id, "markdown": build_intent_context(&intent)})))
}

pub async fn resume_select(
    State(state): State<AppState>,
    JsonBody(request): JsonBody<ResumeSelectRequest>,
) -> ApiResult<Response> {
    let store = state.store.clone();
    let wanted = request.intent_id.clone().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let pool = blocking(move || -> intent_store::Result<Vec<Intent>> {
        match wanted {
            Some(id) => Ok(store.intent_by_id(&id)?.into_iter().collect()),
            None => Ok(store.root_intents(Some(500))?.into_iter().map(|r| r.intent).collect()),
        }
    })
    .await?
    .map_err(|e| ApiError::store(e, BAD))?;
    let runtime = state.llm();
    let use_llm = request.query.as_deref().is_some_and(|q| !q.trim().is_empty()) && state.inference_usable().await;
    let reranker: Option<&dyn Reranker> = if use_llm { runtime.reranker.as_deref().map(|r| r as &dyn Reranker) } else { None };
    let timeout = if runtime.settings.is_local() { Duration::from_secs(30) } else { Duration::from_secs(8) };
    let response = select_resume_preview(&pool, &request, reranker, RerankOptions { timeout }).await;
    match response {
        Some(r) => Ok(Json(r).into_response()),
        None => Err(ApiError::not_found("No stored intent matched the selection")),
    }
}

// ---------------------------------------------------------------------------
// Copilot
// ---------------------------------------------------------------------------

fn copilot_not_configured() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "ok": false,
            "code": "copilot_not_configured",
            "message": "Intent Copilot is not configured. Choose a provider in Settings (a local model server such as Ollama, or a cloud provider with credentials) and enable the copilot.",
        })),
    )
        .into_response()
}

async fn run_copilot(state: &AppState, request: CopilotQueryRequest) -> Response {
    let runtime = state.llm();
    let Some(copilot) = runtime.copilot.clone() else { return copilot_not_configured() };
    if !state.inference_usable().await {
        return copilot_not_configured();
    }
    match copilot.query(&request).await {
        Ok(response) => Json(response).into_response(),
        Err(err) => {
            tracing::warn!(error = %err, "copilot provider request failed");
            ApiError::new(StatusCode::BAD_GATEWAY, "Copilot provider request failed").into_response()
        }
    }
}

pub async fn copilot_query(State(state): State<AppState>, JsonBody(request): JsonBody<CopilotQueryRequest>) -> Response {
    if request.mode == CopilotMode::Narrative {
        let (Some(from), Some(to)) = (request.date_from.as_deref(), request.date_to.as_deref()) else {
            return ApiError::bad_request("narrative mode requires date_from and date_to").into_response();
        };
        if let Err(e) = check_date(from).and_then(|()| check_date(to)) {
            return e.into_response();
        }
        if from > to {
            return ApiError::bad_request("date_from must not be later than date_to").into_response();
        }
    }
    run_copilot(&state, request).await
}

pub async fn copilot_briefing(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let request = CopilotQueryRequest {
        question: "Summarize this intent for resume".into(),
        date_from: None,
        date_to: None,
        mode: CopilotMode::Briefing,
        intent_id: Some(id),
        conversation_id: None,
    };
    run_copilot(&state, request).await
}

// ---------------------------------------------------------------------------
// Pipeline and memory
// ---------------------------------------------------------------------------

fn pipeline_error(err: anyhow::Error) -> ApiError {
    match err.downcast::<intent_store::StoreError>() {
        Ok(e) => ApiError::store(e, BAD),
        Err(other) => {
            tracing::error!(error = %other, "pipeline run failed");
            ApiError::internal("pipeline run failed")
        }
    }
}

pub async fn pipeline_run(State(state): State<AppState>, QueryParams(q): QueryParams<OptDate>) -> ApiResult<Response> {
    let date = match q.date {
        Some(d) => {
            check_date(&d)?;
            d
        }
        None => util::today(&state.store),
    };
    state.dirty_since.store(0, std::sync::atomic::Ordering::Relaxed);
    let result = pipeline::run(&state, &date, None, RunMode::Cached).await.map_err(pipeline_error)?;
    Ok(Json(result).into_response())
}

pub async fn pipeline_replay(State(state): State<AppState>, JsonBody(export): JsonBody<DayExport>) -> ApiResult<Response> {
    check_date(&export.date)?;
    let result = pipeline::run(&state, &export.date, Some(export.events), RunMode::Cached).await.map_err(pipeline_error)?;
    Ok(Json(result).into_response())
}

pub async fn pipeline_recompute(State(state): State<AppState>, QueryParams(q): QueryParams<OptDate>) -> ApiResult<Response> {
    let date = q.date.ok_or_else(|| ApiError::unprocessable("date is required"))?;
    check_date(&date)?;
    let store = state.store.clone();
    let day = date.clone();
    blocking(move || store.delete_date(&day)).await?.map_err(|e| ApiError::store(e, BAD))?;
    let result = pipeline::run(&state, &date, None, RunMode::Force).await.map_err(pipeline_error)?;
    Ok(Json(result).into_response())
}

pub async fn forget_date(State(state): State<AppState>, Path(date): Path<String>) -> ApiResult<Response> {
    check_date(&date)?;
    let store = state.store.clone();
    let report = blocking(move || store.delete_date(&date)).await?.map_err(|e| ApiError::store(e, BAD))?;
    state.prediction.invalidate();
    Ok(Json(report).into_response())
}

pub async fn forget_project(State(state): State<AppState>, Path(project): Path<String>) -> ApiResult<Response> {
    if project.trim().is_empty() {
        return Err(ApiError::bad_request("project must be non-empty"));
    }
    let store = state.store.clone();
    let report = blocking(move || store.delete_project(&project)).await?.map_err(|e| ApiError::store(e, BAD))?;
    state.prediction.invalidate();
    Ok(Json(report).into_response())
}
