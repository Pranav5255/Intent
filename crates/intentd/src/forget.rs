//! Forget a day or a project.
//!
//! Both first flush the ingest queue, so events accepted before the request are
//! deleted too. Both hold the pipeline lock, so a run that read the events before
//! the forget cannot save its intents after it.
//!
//! A forget is a point in time. Events that arrive afterwards are new activity and
//! are recorded as usual: the user who forgets today and keeps working gets a new
//! timeline, not a blind spot. To stop capturing something, the user pauses capture,
//! blocks a domain or removes a workspace. So there is no tombstone to check on
//! every ingest.

use intent_engine::enrich::event_votes_for_project;
use intent_store::DeleteReport;

use crate::jobs;
use crate::pipeline::{self, RunMode};
use crate::state::Shared;

/// Delete the day's raw events, stats, intents, search rows and vectors, and its
/// scheduler state.
pub async fn forget_date(state: &Shared, date: &str) -> anyhow::Result<DeleteReport> {
    state.writer.flush().await;
    let _lock = state.pipeline_lock.lock().await;
    let store = state.store.clone();
    let day = date.to_string();
    let report = tokio::task::spawn_blocking(move || -> intent_store::Result<DeleteReport> {
        let report = store.forget_date(&day)?;
        jobs::reset_schedule(&store, &report.affected_dates)?;
        Ok(report)
    })
    .await??;
    clear_caches(state);
    Ok(report)
}

/// Delete the raw events the engine attributes to `project` and the project's
/// intents, then rebuild every affected date from the events that are left, so
/// sessions that mixed projects lose the forgotten one too.
pub async fn forget_project(state: &Shared, project: &str) -> anyhow::Result<DeleteReport> {
    let name = project.trim();
    let name = name.strip_prefix("project:").unwrap_or(name).trim().to_string();
    state.writer.flush().await;
    let lock = state.pipeline_lock.lock().await;
    let store = state.store.clone();
    let report = tokio::task::spawn_blocking(move || -> intent_store::Result<DeleteReport> {
        // The project name comes from a path segment, so it appears verbatim in the
        // payload of every event that votes for it. The engine decides the rest.
        let ids: Vec<String> = store
            .events_mentioning(&json_fragment(&name))?
            .into_iter()
            .filter(|event| event_votes_for_project(event, &name))
            .map(|event| event.id)
            .collect();
        let report = store.forget_project(&name, &ids)?;
        jobs::reset_schedule(&store, &report.affected_dates)?;
        Ok(report)
    })
    .await??;
    for date in &report.affected_dates {
        // The forget itself is done; a date that fails to rebuild stays empty until
        // its next run.
        if let Err(err) = pipeline::run_locked(state, &lock, date, None, RunMode::Force).await {
            tracing::warn!(error = %err, date = %date, "rebuilding a date after a forget failed");
        }
    }
    clear_caches(state);
    Ok(report)
}

/// In-memory state that may still hold forgotten activity.
fn clear_caches(state: &Shared) {
    state.prediction.invalidate();
    state.current.clear();
}

/// `text` as it appears inside a stored JSON string (quotes and backslashes escaped).
fn json_fragment(text: &str) -> String {
    let quoted = serde_json::Value::String(text.to_string()).to_string();
    quoted.strip_prefix('"').and_then(|q| q.strip_suffix('"')).unwrap_or(&quoted).to_string()
}

#[cfg(test)]
mod tests {
    use super::json_fragment;

    #[test]
    fn json_fragment_escapes_like_the_store() {
        assert_eq!(json_fragment("plain"), "plain");
        assert_eq!(json_fragment("a\"b\\c"), "a\\\"b\\\\c");
        assert_eq!(json_fragment("café"), "café");
    }
}
