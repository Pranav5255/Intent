//! 1.x capture routes (`capture/event_server/main.py`), same paths and shapes.

use std::collections::BTreeMap;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use intent_capture::config::{
    DetailedCaptureUpdate, FilesystemCaptureConfig, FilesystemCaptureUpdate, RetentionPolicy, RetentionPurge,
    WorkspacesConfig, WorkspacesUpdate,
};
use intent_capture::preview::build_preview_sample;
use intent_capture::restore::{self, RestoreRequest, RestoreResult};
use intent_capture::ConfigError;
use intent_store::{RetentionCutoffs, now_ts};
use intent_types::{EventCursor, IngestResult};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::{ApiError, ApiResult, JsonBody, QueryParams};
use crate::ingest::{Delivery, IngestError, Ingested, SubmitError, ingest, parse_event};
use crate::jobs::RETENTION_KEY;
use crate::pause::MAX_PAUSE_SECONDS;
use crate::pipeline::LAST_RUN_KEY;
use crate::state::AppState;
use crate::util;

const UNPROCESSABLE: StatusCode = StatusCode::UNPROCESSABLE_ENTITY;

fn config_read(err: ConfigError) -> ApiError {
    ApiError::internal(err.to_string())
}

fn config_write(err: ConfigError) -> ApiError {
    match err {
        ConfigError::Invalid(m) => ApiError::unprocessable(m),
        ConfigError::Io(m) => ApiError::internal(m),
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> ApiResult<T> {
    Ok(tokio::task::spawn_blocking(f).await?)
}

pub async fn healthz(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "pipeline_version": intent_engine::PIPELINE_VERSION,
        "database": state.store.path(),
    }))
}

// ---------------------------------------------------------------------------
// Ingest
// ---------------------------------------------------------------------------

pub async fn ingest_event(State(state): State<AppState>, body: Bytes) -> Response {
    let event = match parse_event(&body) {
        Ok(event) => event,
        Err(IngestError::Invalid(msg)) => return ApiError::unprocessable(msg).into_response(),
        Err(_) => return ApiError::unprocessable("invalid event").into_response(),
    };
    match ingest(&state, event, Delivery::Wait).await {
        Ok(Ingested::Stored(outcome)) => {
            let status = if outcome.inserted { StatusCode::CREATED } else { StatusCode::OK };
            if outcome.inserted {
                tracing::debug!(source = %outcome.event.source, kind = %outcome.event.kind, "event ingested");
            }
            (status, Json(IngestResult { ok: true, inserted: outcome.inserted, event: outcome.event })).into_response()
        }
        Ok(Ingested::Dropped(_)) | Ok(Ingested::Queued) => StatusCode::NO_CONTENT.into_response(),
        Err(IngestError::Invalid(msg)) => ApiError::unprocessable(msg).into_response(),
        Err(IngestError::Config(msg)) => ApiError::internal(msg).into_response(),
        Err(IngestError::Unavailable(err)) => {
            tracing::error!(?err, "event persistence unavailable");
            let detail = match err {
                SubmitError::QueueFull => "event writer queue is full",
                _ => "event persistence is temporarily unavailable",
            };
            ApiError::unavailable(detail).into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// Event reads / export
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct EventsQuery {
    date: Option<String>,
    since: Option<i64>,
}

pub async fn events(State(state): State<AppState>, QueryParams(q): QueryParams<EventsQuery>) -> ApiResult<Response> {
    if q.date.is_some() && q.since.is_some() {
        return Err(ApiError::unprocessable("date and since cannot be used together"));
    }
    if q.since.is_some_and(|s| s < 0) {
        return Err(ApiError::unprocessable("since must be greater than or equal to 0"));
    }
    let store = state.store.clone();
    let events = blocking(move || match q.date {
        Some(date) => store.events_for_day(&date),
        None => store.events_since(q.since.unwrap_or(0)),
    })
    .await?
    .map_err(|e| ApiError::store(e, UNPROCESSABLE))?;
    Ok(Json(events).into_response())
}

#[derive(Deserialize)]
pub struct DateQuery {
    date: String,
}

pub async fn export_day_meta(State(state): State<AppState>, QueryParams(q): QueryParams<DateQuery>) -> ApiResult<Response> {
    let store = state.store.clone();
    let meta = blocking(move || store.day_meta(&q.date)).await?.map_err(|e| ApiError::store(e, UNPROCESSABLE))?;
    Ok(Json(meta).into_response())
}

pub async fn export_day(State(state): State<AppState>, QueryParams(q): QueryParams<DateQuery>) -> ApiResult<Response> {
    let store = state.store.clone();
    let export = blocking(move || store.export_day(&q.date)).await?.map_err(|e| ApiError::store(e, UNPROCESSABLE))?;
    Ok(Json(export).into_response())
}

#[derive(Deserialize)]
pub struct IncrementalQuery {
    date: String,
    after_ts: Option<i64>,
    after_id: Option<String>,
    limit: Option<usize>,
}

pub async fn events_incremental(
    State(state): State<AppState>,
    QueryParams(q): QueryParams<IncrementalQuery>,
) -> ApiResult<Response> {
    let limit = q.limit.unwrap_or(500);
    if !(1..=2000).contains(&limit) {
        return Err(ApiError::unprocessable("limit must be between 1 and 2000"));
    }
    let cursor = match (q.after_ts, q.after_id) {
        (Some(after_ts), Some(after_id)) if after_ts >= 0 => Some(EventCursor { after_ts, after_id }),
        (None, None) => None,
        (Some(_), Some(_)) => return Err(ApiError::unprocessable("after_ts must be greater than or equal to 0")),
        _ => return Err(ApiError::unprocessable("after_ts and after_id must be provided together")),
    };
    let store = state.store.clone();
    let date = q.date;
    let page = blocking(move || store.events_page(&date, cursor.as_ref(), limit))
        .await?
        .map_err(|e| ApiError::store(e, UNPROCESSABLE))?;
    Ok(Json(json!({
        "version": 1,
        "date": page.date,
        "events": page.events,
        "has_more": page.has_more,
        "next_cursor": page.next_cursor,
        "page_meta": {"revision": page.revision, "content_hash": page.content_hash},
    }))
    .into_response())
}

// ---------------------------------------------------------------------------
// Consent configuration
// ---------------------------------------------------------------------------

pub async fn detailed_get(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    Ok(Json(state.config.public_detailed_config().map_err(config_read)?))
}

pub async fn detailed_put(
    State(state): State<AppState>,
    JsonBody(update): JsonBody<DetailedCaptureUpdate>,
) -> ApiResult<Json<Value>> {
    state.config.update_detailed_capture(&update).map_err(config_write)?;
    state.refresh_watcher();
    Ok(Json(state.config.public_detailed_config().map_err(config_read)?))
}

pub async fn workspaces_get(State(state): State<AppState>) -> ApiResult<Json<WorkspacesConfig>> {
    Ok(Json((*state.config.workspaces().map_err(config_read)?).clone()))
}

/// Replacing the approved workspaces reconfigures the filesystem watcher live.
pub async fn workspaces_put(
    State(state): State<AppState>,
    JsonBody(update): JsonBody<WorkspacesUpdate>,
) -> ApiResult<Json<WorkspacesConfig>> {
    let saved = state.config.set_workspaces(WorkspacesConfig { workspaces: update.workspaces }).map_err(config_write)?;
    state.refresh_watcher();
    Ok(Json((*saved).clone()))
}

pub async fn filesystem_get(State(state): State<AppState>) -> ApiResult<Json<FilesystemCaptureConfig>> {
    Ok(Json(*state.config.filesystem_capture().map_err(config_read)?))
}

pub async fn filesystem_put(
    State(state): State<AppState>,
    JsonBody(update): JsonBody<FilesystemCaptureUpdate>,
) -> ApiResult<Json<FilesystemCaptureConfig>> {
    let saved = state
        .config
        .set_filesystem_capture(FilesystemCaptureConfig { all_accessible: update.all_accessible })
        .map_err(config_write)?;
    state.refresh_watcher();
    Ok(Json(*saved))
}

pub async fn retention_policy_get(State(state): State<AppState>) -> ApiResult<Json<RetentionPolicy>> {
    Ok(Json(*state.config.retention_policy().map_err(config_read)?))
}

pub async fn retention_policy_put(
    State(state): State<AppState>,
    JsonBody(policy): JsonBody<RetentionPolicy>,
) -> ApiResult<Json<RetentionPolicy>> {
    Ok(Json(*state.config.set_retention_policy(policy).map_err(config_write)?))
}

pub async fn preview_sample(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let detailed = state.config.detailed_capture().map_err(config_read)?;
    Ok(Json(build_preview_sample(&detailed)))
}

pub async fn public_config(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let blocklist = state.config.blocklist().map_err(config_read)?;
    Ok(Json(json!({"blocked_domains": blocklist.patterns()})))
}

// ---------------------------------------------------------------------------
// Purges and retention
// ---------------------------------------------------------------------------

pub async fn purge_detailed(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let store = state.store.clone();
    let report = blocking(move || store.purge_detailed_events()).await?.map_err(|e| ApiError::store(e, UNPROCESSABLE))?;
    Ok(Json(json!({"ok": true, "deleted": report.deleted.detailed, "affected_days": report.affected_days})))
}

#[derive(Deserialize)]
pub struct RetentionPreviewQuery {
    detailed_days: Option<i64>,
    metadata_days: Option<i64>,
}

fn cutoffs(detailed_days: Option<i64>, metadata_days: Option<i64>, now: i64) -> RetentionCutoffs {
    RetentionCutoffs {
        detailed_before: detailed_days.map(|d| now - d * 86_400),
        metadata_before: metadata_days.map(|d| now - d * 86_400),
    }
}

fn preview_body(detailed_days: Option<i64>, metadata_days: Option<i64>, now: i64, eligible: Value) -> Value {
    let c = cutoffs(detailed_days, metadata_days, now);
    json!({
        "now": now,
        "detailed_days": detailed_days,
        "metadata_days": metadata_days,
        "cutoffs": {"detailed_before": c.detailed_before, "metadata_before": c.metadata_before},
        "eligible": eligible,
    })
}

pub async fn retention_preview(
    State(state): State<AppState>,
    QueryParams(q): QueryParams<RetentionPreviewQuery>,
) -> ApiResult<Json<Value>> {
    let check = RetentionPurge { detailed_days: q.detailed_days, metadata_days: q.metadata_days, confirm: false };
    check.validate().map_err(config_write)?;
    let now = now_ts();
    let c = cutoffs(q.detailed_days, q.metadata_days, now);
    let store = state.store.clone();
    let counts = blocking(move || store.retention_preview(&c)).await?.map_err(|e| ApiError::store(e, UNPROCESSABLE))?;
    Ok(Json(preview_body(q.detailed_days, q.metadata_days, now, json!(counts))))
}

pub async fn retention_purge(State(state): State<AppState>, JsonBody(body): JsonBody<RetentionPurge>) -> ApiResult<Json<Value>> {
    body.validate().map_err(config_write)?;
    if !body.confirm {
        return Err(ApiError::new(StatusCode::CONFLICT, "set confirm=true after reviewing retention preview"));
    }
    let now = now_ts();
    let c = cutoffs(body.detailed_days, body.metadata_days, now);
    let store = state.store.clone();
    let (preview, report) = blocking(move || -> intent_store::Result<_> {
        let preview = store.retention_preview(&c)?;
        let report = store.purge_retention(&c)?;
        Ok((preview, report))
    })
    .await?
    .map_err(|e| ApiError::store(e, UNPROCESSABLE))?;
    let mut out = preview_body(body.detailed_days, body.metadata_days, now, json!(preview));
    out["ok"] = json!(true);
    out["deleted"] = json!(report.deleted);
    out["affected_days"] = json!(report.affected_days);
    Ok(Json(out))
}

// ---------------------------------------------------------------------------
// Restore and pause
// ---------------------------------------------------------------------------

pub async fn restore(State(state): State<AppState>, JsonBody(body): JsonBody<RestoreRequest>) -> ApiResult<Json<RestoreResult>> {
    let (payload, mode) = body.validate().map_err(|e| ApiError::unprocessable(e.message))?;
    let probe = state.restore_probe.clone();
    let spawner = state.restore_spawner.clone();
    let result = blocking(move || {
        let plan = match &probe {
            Some(probe) => restore::plan_restore_with(&payload, mode, probe.as_ref()),
            None => restore::plan_restore(&payload, mode),
        };
        restore::execute_with(&plan, |cmd| spawner(cmd))
    })
    .await?;
    Ok(Json(result))
}

#[derive(Deserialize)]
pub struct PauseBody {
    paused: bool,
    #[serde(default)]
    duration_seconds: Option<i64>,
    /// Absolute resume time (epoch seconds), as the Tauri app sends it.
    #[serde(default)]
    until: Option<i64>,
}

pub async fn pause(State(state): State<AppState>, JsonBody(mut body): JsonBody<PauseBody>) -> ApiResult<Json<Value>> {
    if body.duration_seconds.is_none()
        && let Some(until) = body.until
    {
        body.duration_seconds = Some((until - now_ts()).max(1));
    }
    if let Some(s) = body.duration_seconds
        && !(1..=MAX_PAUSE_SECONDS).contains(&s)
    {
        return Err(ApiError::unprocessable(format!("duration_seconds must be between 1 and {MAX_PAUSE_SECONDS}")));
    }
    let st = state.clone();
    let record = blocking(move || st.pause.set(&st.store, body.paused, body.duration_seconds))
        .await?
        .map_err(|e| ApiError::store(e, UNPROCESSABLE))?;
    tracing::info!(paused = record.paused, until = ?record.until, "capture pause changed");
    Ok(Json(json!({"ok": true, "paused": record.paused, "paused_until": record.until, "until": record.until})))
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

const SOURCES: &[&str] = &["vscode", "firefox", "shell", "linux", "filesystem"];

fn stale_after() -> i64 {
    std::env::var("INTENT_SOURCE_STALE_AFTER_SECONDS").ok().and_then(|v| v.parse().ok()).filter(|v| *v > 0).unwrap_or(1800)
}

pub async fn status(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let store = state.store.clone();
    let (stats, detailed_counts, last_run, retention) = blocking(move || -> intent_store::Result<_> {
        Ok((
            store.source_stats()?,
            store.detailed_event_counts()?,
            store.get_metadata(LAST_RUN_KEY)?,
            store.get_metadata(RETENTION_KEY)?,
        ))
    })
    .await?
    .map_err(|e| ApiError::store(e, StatusCode::INTERNAL_SERVER_ERROR))?;

    let now = now_ts();
    let stale = stale_after();
    let mut sources: BTreeMap<String, Value> = SOURCES
        .iter()
        .map(|s| {
            (s.to_string(), json!({"event_count": 0, "last_event_ts": null, "last_ingested_at": null,
                                   "last_ingest_lag_seconds": null, "healthy": false}))
        })
        .collect();
    for s in stats {
        let lag = match (s.last_event_ts, s.last_ingested_at) {
            (Some(ev), Some(ing)) => Some((ing - ev).max(0)),
            _ => None,
        };
        sources.insert(
            s.source.clone(),
            json!({
                "event_count": s.event_count,
                "last_event_ts": s.last_event_ts,
                "last_ingested_at": s.last_ingested_at,
                "last_ingest_lag_seconds": lag,
                "healthy": s.last_event_ts.is_some_and(|ts| now - ts <= stale),
            }),
        );
    }

    let collectors: Vec<_> = state.collectors.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|h| h.snapshot()).collect();
    let collector_running = |name: &str| collectors.iter().any(|c| c.name == name && c.running);
    let healthy = |name: &str| sources.get(name).and_then(|v| v["healthy"].as_bool()).unwrap_or(false);

    let mut detailed = state.config.public_detailed_config().map_err(config_read)?;
    detailed["event_counts"] = json!(detailed_counts);
    let pause = state.pause.current();
    let db_size = util::database_size(state.store.path());
    let probe = state.local_probe(false).await;
    let llm = state.llm();
    let parse = |raw: Option<String>| raw.and_then(|r| serde_json::from_str::<Value>(&r).ok());

    Ok(Json(json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "session_type": std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".into()),
        "capture_paused": pause.paused,
        "paused_until": pause.until,
        "capture_paused_until": pause.until,
        "ingestion": state.writer.snapshot(),
        "sources": sources,
        "services": {
            "event_server": true,
            "engine": true,
            "x11_tracker": collector_running("focus") || healthy("linux"),
            "workspace_watch": collector_running("filesystem") || healthy("filesystem"),
        },
        "collectors": collectors,
        "activity": state.feed.snapshot(),
        "detailed_capture": detailed,
        "llm": llm.status(probe.as_ref()),
        "database": {"path": state.store.path(), "size_bytes": db_size},
        "db_size_bytes": db_size,
        "pipeline": {"last_run": parse(last_run)},
        "retention": {"policy": state.config.retention_policy().ok().map(|p| *p), "last_run": parse(retention)},
        "uptime_seconds": now - state.started_at,
    })))
}
