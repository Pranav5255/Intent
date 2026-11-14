//! `/settings/llm*`: summary, update (atomic provider rebuild) and Gemini credentials.

use axum::Json;
use axum::extract::State;
use intent_llm::LlmSettings;
use intent_llm::settings::{SettingsError, save_gemini_credentials};
use intent_types::{LlmSettingsSummary, LlmSettingsUpdate};
use serde::Deserialize;

use crate::error::{ApiError, ApiResult, JsonBody};
use crate::state::AppState;

fn settings_error(err: SettingsError) -> ApiError {
    match err {
        SettingsError::Io(e) => {
            tracing::error!(error = %e, "could not write LLM settings");
            ApiError::internal("could not write LLM settings")
        }
        other => ApiError::bad_request(other.to_string()),
    }
}

async fn summary(state: &AppState, force_probe: bool) -> LlmSettingsSummary {
    let probe = state.local_probe(force_probe).await;
    state.llm().settings.summary(probe.map(|p| p.reachable))
}

pub async fn get_llm(State(state): State<AppState>) -> Json<LlmSettingsSummary> {
    Json(summary(&state, false).await)
}

/// Persist, then rebuild every provider and swap them in atomically.
async fn persist_and_install(state: &AppState, next: LlmSettings) -> ApiResult<()> {
    let path = LlmSettings::path(&state.paths.config_dir());
    let to_save = next.clone();
    tokio::task::spawn_blocking(move || to_save.save(&path)).await?.map_err(settings_error)?;
    state.install_llm(next);
    Ok(())
}

pub async fn put_llm(
    State(state): State<AppState>,
    JsonBody(update): JsonBody<LlmSettingsUpdate>,
) -> ApiResult<Json<LlmSettingsSummary>> {
    let mut next = state.llm().settings.clone();
    next.apply_update(update).map_err(settings_error)?;
    persist_and_install(&state, next).await?;
    tracing::info!("LLM settings updated; providers rebuilt");
    Ok(Json(summary(&state, true).await))
}

#[derive(Deserialize)]
pub struct CredentialsBody {
    credentials: Option<String>,
}

pub async fn post_gemini_credentials(
    State(state): State<AppState>,
    JsonBody(body): JsonBody<CredentialsBody>,
) -> ApiResult<Json<LlmSettingsSummary>> {
    let raw = body
        .credentials
        .filter(|c| !c.trim().is_empty())
        .ok_or_else(|| ApiError::bad_request("credentials must be a non-empty JSON string"))?;
    let dir = state.paths.config_dir();
    let (path, key) = tokio::task::spawn_blocking(move || save_gemini_credentials(&dir, raw.as_bytes()))
        .await?
        .map_err(settings_error)?;
    let mut next = state.llm().settings.clone();
    next.gemini_credentials_path = Some(path);
    if next.google_cloud_project.as_deref().is_none_or(|p| p.trim().is_empty()) {
        next.google_cloud_project = Some(key.project_id);
    }
    persist_and_install(&state, next).await?;
    Ok(Json(summary(&state, false).await))
}

pub async fn delete_gemini_credentials(State(state): State<AppState>) -> ApiResult<Json<LlmSettingsSummary>> {
    let mut next = state.llm().settings.clone();
    if let Some(path) = next.gemini_credentials_path.take() {
        let _ = tokio::task::spawn_blocking(move || std::fs::remove_file(path)).await;
    }
    persist_and_install(&state, next).await?;
    Ok(Json(summary(&state, false).await))
}
