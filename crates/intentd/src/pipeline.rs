//! Running the intent engine against stored events and persisting the result.

use intent_engine::pipeline::{PipelineInput, source_hash};
use intent_store::now_ts;
use intent_types::{PipelineResult, RawEvent};
use serde::{Deserialize, Serialize};

use crate::state::Shared;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// Reuse the cached run when the input hash matches.
    Cached,
    /// Always recompute.
    Force,
}

/// Outcome summary persisted for `/v1/status` and the scheduler.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub date: String,
    pub outcome: String,
    pub at: i64,
    pub duration_ms: u64,
    pub event_count: usize,
    pub intent_count: usize,
    pub cached: bool,
    pub labeler: String,
    #[serde(default)]
    pub error: Option<String>,
}

pub const LAST_RUN_KEY: &str = "pipeline_last_run";

/// Run the pipeline for `date` over `events` (the stored day when `None`).
pub async fn run(
    shared: &Shared,
    date: &str,
    events: Option<Vec<RawEvent>>,
    mode: RunMode,
) -> anyhow::Result<PipelineResult> {
    let _guard = shared.pipeline_lock.lock().await;
    let started = std::time::Instant::now();
    let events = match events {
        Some(events) => events,
        None => {
            let store = shared.store.clone();
            let day = date.to_string();
            tokio::task::spawn_blocking(move || store.events_for_day(&day)).await??
        }
    };
    let event_count = events.len();
    let usable = shared.inference_usable().await;
    let runtime = shared.llm();
    let deps = runtime.pipeline_deps(usable);
    let labeler = deps.label_provider.cache_identity();
    let input = PipelineInput { date: date.to_string(), events };

    if mode == RunMode::Cached {
        let hash = source_hash(&input, &deps);
        let store = shared.store.clone();
        let (day, key) = (date.to_string(), hash.clone());
        if let Some(cached) = tokio::task::spawn_blocking(move || store.cached_run(&day, &key)).await?? {
            let result = PipelineResult {
                intents: cached.intents,
                warnings: cached.warnings,
                source_hash: hash,
                pipeline_version: intent_engine::PIPELINE_VERSION.to_string(),
                cached: true,
            };
            record(shared, date, "unchanged", started, event_count, &result, &labeler, None).await;
            return Ok(result);
        }
    }

    let result = intent_engine::run(input, &deps).await;
    let store = shared.store.clone();
    let day = date.to_string();
    let to_save = result.clone();
    tokio::task::spawn_blocking(move || store.save_pipeline_run(&day, &to_save)).await??;
    shared.prediction.invalidate();
    shared.embed_wanted.notify_one();
    record(shared, date, "success", started, event_count, &result, &labeler, None).await;
    tracing::info!(
        date,
        events = event_count,
        intents = result.intents.len(),
        warnings = result.warnings.len(),
        labeler = %labeler,
        ms = started.elapsed().as_millis() as u64,
        "pipeline run complete"
    );
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
async fn record(
    shared: &Shared,
    date: &str,
    outcome: &str,
    started: std::time::Instant,
    event_count: usize,
    result: &PipelineResult,
    labeler: &str,
    error: Option<String>,
) {
    let rec = RunRecord {
        date: date.to_string(),
        outcome: outcome.to_string(),
        at: now_ts(),
        duration_ms: started.elapsed().as_millis() as u64,
        event_count,
        intent_count: result.intents.len(),
        cached: result.cached,
        labeler: labeler.to_string(),
        error,
    };
    save_record(shared, &rec).await;
}

pub async fn save_record(shared: &Shared, rec: &RunRecord) {
    let store = shared.store.clone();
    let Ok(json) = serde_json::to_string(rec) else { return };
    let _ = tokio::task::spawn_blocking(move || store.set_metadata(LAST_RUN_KEY, &json)).await;
}
