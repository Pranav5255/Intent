//! Background jobs. Everything runs on one housekeeping timer (one wake-up per
//! minute) plus an event-driven embedding backfill, so an idle daemon costs ~0 CPU.
//!
//! * Scheduler (replaces the 0.1 3-hourly systemd timer): runs today's pipeline when
//!   new events arrived AND (the user has been idle ≥ 2 min OR 30 min passed since the
//!   last run); skips when the day's content hash is unchanged. Yesterday is finalized
//!   once after midnight. Outcomes are persisted (`pipeline_last_run`).
//! * Retention: applied ~30 s after start and then daily (0.1 never applied the
//!   policy automatically).
//! * Pause expiry: persists the automatic resume of a timed pause.
//! * "Continue yesterday?" notification once per morning (Linux, see `notify`).
//! * Embedding backfill for root intents without vectors.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use intent_store::{EmbedderSpec, RetentionCutoffs, now_ts};
use serde_json::json;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::llm::is_offline_embedder;
use crate::pipeline::{self, RunMode, RunRecord};
use crate::state::{AppState, Shared};
use crate::util;

pub const IDLE_BEFORE_RUN_MS: u64 = 2 * 60 * 1000;
pub const MAX_STALENESS: Duration = Duration::from_secs(30 * 60);
const TICK: Duration = Duration::from_secs(60);
const RETENTION_FIRST_RUN: Duration = Duration::from_secs(30);
const RETENTION_PERIOD: Duration = Duration::from_secs(24 * 3600);
const SCHED_HASH_KEY: &str = "scheduler_day_hash";
const FINALIZED_KEY: &str = "scheduler_finalized_date";
pub const RETENTION_KEY: &str = "retention_last_run";

pub fn spawn(state: &AppState) -> Vec<JoinHandle<()>> {
    vec![tokio::spawn(housekeeping(Arc::clone(state))), tokio::spawn(embedding_backfill(Arc::clone(state)))]
}

/// Scheduler decision (pure): new events are pending and either the user is idle or
/// the last run is too old.
pub fn should_run(dirty_since: i64, idle_ms: u64, since_last_run: Duration) -> bool {
    dirty_since > 0 && (idle_ms >= IDLE_BEFORE_RUN_MS || since_last_run >= MAX_STALENESS)
}

async fn housekeeping(state: AppState) {
    let start = Instant::now();
    let mut last_retention: Option<Instant> = None;
    let mut last_run = Instant::now();
    let mut ticker = tokio::time::interval_at(start + RETENTION_FIRST_RUN, TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = state.cancel.cancelled() => break,
            _ = ticker.tick() => {}
        }
        {
            let st = Arc::clone(&state);
            let _ = tokio::task::spawn_blocking(move || st.pause.expire_if_due(&st.store)).await;
        }
        if last_retention.is_none_or(|t| t.elapsed() >= RETENTION_PERIOD) {
            last_retention = Some(Instant::now());
            enforce_retention(&state).await;
        }
        if should_run(state.dirty_since.load(Ordering::Relaxed), state.feed.idle_ms(), last_run.elapsed()) {
            last_run = Instant::now();
            run_today(&state).await;
        }
        finalize_yesterday(&state).await;
        #[cfg(target_os = "linux")]
        crate::notify::maybe_continue_yesterday(&state).await;
    }
}

/// Drop the scheduler's content hash for `dates`, so new events on a forgotten day
/// are processed again.
pub fn reset_schedule(store: &intent_store::Store, dates: &[String]) -> intent_store::Result<()> {
    for date in dates {
        store.delete_metadata(&format!("{SCHED_HASH_KEY}:{date}"))?;
    }
    Ok(())
}

async fn meta(state: &Shared, key: &str) -> Option<String> {
    let store = state.store.clone();
    let key = key.to_string();
    tokio::task::spawn_blocking(move || store.get_metadata(&key).ok().flatten()).await.ok().flatten()
}

async fn set_meta(state: &Shared, key: &str, value: String) {
    let store = state.store.clone();
    let key = key.to_string();
    let _ = tokio::task::spawn_blocking(move || store.set_metadata(&key, &value)).await;
}

async fn content_hash(state: &Shared, date: &str) -> Option<(String, i64)> {
    let store = state.store.clone();
    let date = date.to_string();
    tokio::task::spawn_blocking(move || store.day_meta(&date).ok().map(|m| (m.content_hash, m.event_count)))
        .await
        .ok()
        .flatten()
}

/// Incremental run for today, skipped when the day's content hash is unchanged.
pub async fn run_today(state: &Shared) {
    state.dirty_since.store(0, Ordering::Relaxed);
    let today = util::today(&state.store);
    let Some((hash, count)) = content_hash(state, &today).await else { return };
    let key = format!("{SCHED_HASH_KEY}:{today}");
    if count == 0 || meta(state, &key).await.as_deref() == Some(hash.as_str()) {
        return;
    }
    match pipeline::run(state, &today, None, RunMode::Cached).await {
        Ok(_) => set_meta(state, &key, hash).await,
        Err(err) => {
            tracing::warn!(error = %err, date = %today, "scheduled pipeline run failed");
            pipeline::save_record(
                state,
                &RunRecord {
                    date: today,
                    outcome: "pipeline_error".into(),
                    at: now_ts(),
                    error: Some(err.to_string()),
                    ..Default::default()
                },
            )
            .await;
        }
    }
}

/// Process yesterday once after midnight so its late events are included.
async fn finalize_yesterday(state: &Shared) {
    let yesterday = util::yesterday(&state.store);
    if meta(state, FINALIZED_KEY).await.as_deref() == Some(yesterday.as_str()) {
        return;
    }
    if let Some((hash, count)) = content_hash(state, &yesterday).await
        && count > 0
    {
        let key = format!("{SCHED_HASH_KEY}:{yesterday}");
        if meta(state, &key).await.as_deref() != Some(hash.as_str()) {
            if let Err(err) = pipeline::run(state, &yesterday, None, RunMode::Cached).await {
                tracing::warn!(error = %err, date = %yesterday, "finalizing yesterday failed");
                return;
            }
            set_meta(state, &key, hash).await;
        }
    }
    set_meta(state, FINALIZED_KEY, yesterday).await;
}

/// Apply the saved retention policy. Returns the purge report when it ran.
pub async fn enforce_retention(state: &Shared) -> Option<intent_store::PurgeReport> {
    let policy = state.config.retention_policy().ok()?;
    if policy.metadata_days.is_none() && policy.detailed_days.is_none() {
        return None;
    }
    let days = |d: Option<i64>| d.map(|v| v.clamp(1, i64::from(u32::MAX)) as u32);
    let cutoffs = RetentionCutoffs::from_days(days(policy.detailed_days), days(policy.metadata_days), now_ts());
    let store = state.store.clone();
    // A run in progress must not save intents of days the purge removes.
    let _lock = state.pipeline_lock.lock().await;
    match tokio::task::spawn_blocking(move || store.purge_retention(&cutoffs)).await {
        Ok(Ok(report)) => {
            if report.deleted.total > 0 {
                tracing::info!(deleted = report.deleted.total, days = report.affected_days.len(), "retention applied");
            }
            let rec = json!({"at": now_ts(), "deleted": report.deleted, "affected_days": report.affected_days});
            set_meta(state, RETENTION_KEY, rec.to_string()).await;
            Some(report)
        }
        Ok(Err(err)) => {
            tracing::warn!(error = %err, "retention purge failed");
            None
        }
        Err(_) => None,
    }
}

async fn embedding_backfill(state: AppState) {
    // Give startup (migrations, legacy import, collectors) a moment first.
    tokio::select! {
        _ = state.cancel.cancelled() => return,
        _ = tokio::time::sleep(Duration::from_secs(5)) => {}
    }
    loop {
        let n = backfill_once(&state).await;
        if n > 0 {
            tracing::info!(embedded = n, "intent embeddings backfilled");
        }
        tokio::select! {
            _ = state.cancel.cancelled() => break,
            _ = state.embed_wanted.notified() => {}
            _ = tokio::time::sleep(Duration::from_secs(15 * 60)) => {}
        }
    }
}

/// Embed root intents missing a vector with the configured embedder. Returns how
/// many vectors were stored.
pub async fn backfill_once(state: &Shared) -> usize {
    let runtime = state.llm();
    let embedder = Arc::clone(&runtime.providers.embedder);
    if !is_offline_embedder(embedder.as_ref())
        && let Some(probe) = state.local_probe(false).await
        && !probe.reachable
    {
        return 0;
    }
    // Make sure the vector table matches this embedder (switching drops old vectors).
    let identity = embedder.cache_identity();
    let configured = {
        let store = state.store.clone();
        tokio::task::spawn_blocking(move || store.embedder()).await.ok().and_then(Result::ok).flatten()
    };
    if configured.as_ref().map(|s| s.identity.as_str()) != Some(identity.as_str()) {
        let mut dims = embedder.dims();
        if dims == 0 {
            match tokio::time::timeout(Duration::from_secs(30), embedder.embed(&["intent".to_string()])).await {
                Ok(Ok(v)) if !v.is_empty() => dims = v[0].len(),
                _ => return 0,
            }
        }
        let store = state.store.clone();
        let spec = EmbedderSpec { identity, dims };
        if let Ok(Err(err)) = tokio::task::spawn_blocking(move || store.configure_embeddings(&spec)).await {
            tracing::warn!(error = %err, "could not configure embeddings");
            return 0;
        }
    }
    let mut total = 0;
    loop {
        if state.cancel.is_cancelled() {
            break;
        }
        let store = state.store.clone();
        let todo = match tokio::task::spawn_blocking(move || store.intents_missing_embeddings(32)).await {
            Ok(Ok(todo)) if !todo.is_empty() => todo,
            _ => break,
        };
        let texts: Vec<String> = todo.iter().map(|t| format!("{}. {}", t.label, t.summary)).collect();
        let vectors = match tokio::time::timeout(Duration::from_secs(120), embedder.embed(&texts)).await {
            Ok(Ok(v)) if v.len() == todo.len() => v,
            Ok(Err(err)) => {
                tracing::debug!(error = %err, "embedding backfill paused");
                break;
            }
            _ => break,
        };
        let store = state.store.clone();
        let ids: Vec<String> = todo.into_iter().map(|t| t.id).collect();
        let stored = tokio::task::spawn_blocking(move || {
            ids.iter().zip(vectors).filter(|(id, v)| store.upsert_embedding(id, v).unwrap_or(false)).count()
        })
        .await
        .unwrap_or(0);
        if stored == 0 {
            break;
        }
        total += stored;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduler_waits_for_idle_or_staleness() {
        let fresh = Duration::from_secs(60);
        assert!(!should_run(0, IDLE_BEFORE_RUN_MS * 10, MAX_STALENESS * 2), "no new events");
        assert!(!should_run(5, 1_000, fresh), "user active and last run recent");
        assert!(should_run(5, IDLE_BEFORE_RUN_MS, fresh), "idle for two minutes");
        assert!(should_run(5, 0, MAX_STALENESS), "thirty minutes since the last run");
    }
}
