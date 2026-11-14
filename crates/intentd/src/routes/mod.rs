//! HTTP surface: 1.x capture routes (+ unprefixed aliases), engine routes, settings,
//! auth matrix and CORS.
//!
//! ## Auth matrix
//!
//! Open (no token) — exactly what the browser/editor companions and shell hook use:
//! `GET /healthz`, `POST /v1/event` (+ `/event`), `GET /v1/detailed-capture/config`,
//! `GET /v1/config`, and CORS preflights. Every other route (all reads of captured
//! data or intents, all writes, restore, pause, settings, pipeline, copilot, memory
//! deletion) requires `Authorization: Bearer <token>`.

mod capture;
mod engine;
mod settings;

use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{delete, get, post};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::auth;
use crate::state::AppState;

/// `(method, path)` pairs reachable without the bearer token.
pub const OPEN_ROUTES: &[(&str, &str)] = &[
    ("GET", "/healthz"),
    ("POST", "/v1/event"),
    ("POST", "/event"),
    ("GET", "/v1/detailed-capture/config"),
    ("GET", "/v1/config"),
];

pub fn is_open(method: &Method, path: &str) -> bool {
    method == Method::OPTIONS || OPEN_ROUTES.iter().any(|(m, p)| method.as_str() == *m && path == *p)
}

async fn auth_layer(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if is_open(request.method(), request.uri().path()) || auth::authorized(&state.token, request.headers()) {
        next.run(request).await
    } else {
        auth::unauthorized()
    }
}

/// Origins allowed to call the API from a browser context: the Firefox companion
/// (`INTENT_FIREFOX_EXTENSION_ORIGIN`) and the Tauri app. Dev builds also allow the
/// Tauri dev server; release builds never allow Vite/localhost dev origins.
pub fn cors_layer(firefox_origin: Option<&str>) -> CorsLayer {
    let mut origins: Vec<HeaderValue> =
        vec![HeaderValue::from_static("tauri://localhost"), HeaderValue::from_static("http://tauri.localhost")];
    if let Some(origin) = firefox_origin.map(str::trim).filter(|o| !o.is_empty())
        && let Ok(value) = HeaderValue::from_str(origin)
    {
        origins.push(value);
    }
    if cfg!(debug_assertions) {
        origins.push(HeaderValue::from_static("http://localhost:1420"));
        origins.push(HeaderValue::from_static("http://127.0.0.1:1420"));
    }
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION])
}

pub fn router(state: AppState, cors: CorsLayer) -> Router {
    let capture = Router::new()
        .route("/healthz", get(capture::healthz))
        .route("/v1/event", post(capture::ingest_event))
        .route("/event", post(capture::ingest_event))
        .route("/v1/events", get(capture::events))
        .route("/events", get(capture::events))
        .route("/v1/export/day/meta", get(capture::export_day_meta))
        .route("/export/day/meta", get(capture::export_day_meta))
        .route("/v1/events/incremental", get(capture::events_incremental))
        .route("/events/incremental", get(capture::events_incremental))
        .route("/v1/export/day", get(capture::export_day))
        .route("/export/day", get(capture::export_day))
        .route("/v1/detailed-capture/config", get(capture::detailed_get).put(capture::detailed_put))
        .route("/v1/workspaces", get(capture::workspaces_get).put(capture::workspaces_put))
        .route("/v1/filesystem-capture", get(capture::filesystem_get).put(capture::filesystem_put))
        .route("/v1/retention/policy", get(capture::retention_policy_get).put(capture::retention_policy_put))
        .route("/v1/intelligence/preview-sample", get(capture::preview_sample))
        .route("/v1/config", get(capture::public_config))
        .route("/v1/detailed-capture/purge", post(capture::purge_detailed))
        .route("/v1/retention/preview", get(capture::retention_preview))
        .route("/retention/preview", get(capture::retention_preview))
        .route("/v1/retention/purge", post(capture::retention_purge))
        .route("/retention/purge", post(capture::retention_purge))
        .route("/v1/restore", post(capture::restore))
        .route("/restore", post(capture::restore))
        .route("/v1/capture/pause", post(capture::pause))
        .route("/v1/status", get(capture::status))
        .route("/status", get(capture::status));

    let engine = Router::new()
        .route("/intents/yesterday", get(engine::yesterday))
        .route("/intents", get(engine::by_date))
        .route("/intents/search", get(engine::search))
        .route("/intents/stats", get(engine::stats))
        .route("/intents/digest", get(engine::digest))
        .route("/intents/current", get(engine::current))
        .route("/intents/prediction", get(engine::prediction))
        .route("/intents/{id}", get(engine::by_id))
        .route("/intents/{id}/context", get(engine::context))
        .route("/resume/select", post(engine::resume_select))
        .route("/copilot/query", post(engine::copilot_query))
        .route("/copilot/briefing/{id}", get(engine::copilot_briefing))
        .route("/pipeline/run", post(engine::pipeline_run))
        .route("/pipeline/run-replay", post(engine::pipeline_replay))
        .route("/pipeline/recompute", post(engine::pipeline_recompute))
        .route("/v1/memory/date/{date}", delete(engine::forget_date))
        .route("/v1/memory/project/{project}", delete(engine::forget_project))
        .route("/settings/llm", get(settings::get_llm).put(settings::put_llm))
        .route(
            "/settings/llm/gemini-credentials",
            post(settings::post_gemini_credentials).delete(settings::delete_gemini_credentials),
        );

    capture
        .merge(engine)
        .layer(middleware::from_fn_with_state(Arc::clone(&state), auth_layer))
        .layer(cors)
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_routes_are_exactly_the_companion_surface() {
        assert!(is_open(&Method::POST, "/v1/event"));
        assert!(is_open(&Method::GET, "/v1/detailed-capture/config"));
        assert!(!is_open(&Method::PUT, "/v1/detailed-capture/config"));
        assert!(!is_open(&Method::GET, "/v1/events"));
        assert!(!is_open(&Method::POST, "/v1/restore"));
        assert!(!is_open(&Method::GET, "/intents/search"));
        assert!(is_open(&Method::OPTIONS, "/v1/restore"));
    }
}
