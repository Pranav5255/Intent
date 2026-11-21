//! In-process activity collectors and their forwarder into the shared ingest path.

use std::path::Path;
use std::sync::Arc;

use intent_collectors::{
    CollectorHandle, FilesystemCollector, FocusCollector, FocusConfig, FsOptions, IdleCollector, IdleConfig, spawn,
};
use intent_types::RawEvent;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::ingest::{Delivery, IngestError, ingest, revalidate};
use crate::state::AppState;

fn is_sensitive(path: &Path) -> bool {
    intent_capture::secrets::is_excluded_content_path(path)
}

pub struct Spawned {
    pub handles: Vec<CollectorHandle>,
    pub forwarder: JoinHandle<()>,
}

/// Start focus, idle and filesystem collectors (each reports `unavailable` instead of
/// failing when its backend is missing) plus the forwarder task.
pub fn start(state: &AppState) -> Spawned {
    let (tx, rx) = mpsc::channel::<RawEvent>(256);
    let cancel = state.cancel.child_token();
    let feed = Arc::clone(&state.feed);
    // Intent's own writes (database, logs, token, configs) must never feed the watcher.
    let mut never_watch = vec![state.paths.data_dir(), state.paths.config_dir()];
    never_watch.extend(state.paths.runtime_dir());
    let fs_options = FsOptions { never_watch, ..FsOptions::default() };
    let handles = vec![
        spawn(FocusCollector::new(FocusConfig::default(), Some(Arc::clone(&feed))), tx.clone(), cancel.clone()),
        spawn(IdleCollector::new(IdleConfig::default(), Arc::clone(&feed)), tx.clone(), cancel.clone()),
        spawn(FilesystemCollector::new(state.fs_watch.subscribe(), Arc::new(is_sensitive), fs_options), tx, cancel),
    ];
    {
        let mut statuses = state.collectors.lock().unwrap_or_else(|e| e.into_inner());
        statuses.extend(handles.iter().map(|h| h.status.clone()));
    }
    let forwarder = tokio::spawn(forward(Arc::clone(state), rx));
    Spawned { handles, forwarder }
}

/// Collector events go through exactly the same validation, pause, consent,
/// redaction and blocklist path as HTTP events.
pub async fn forward(state: AppState, mut rx: mpsc::Receiver<RawEvent>) {
    while let Some(event) = rx.recv().await {
        let event = match revalidate(event) {
            Ok(event) => event,
            Err(IngestError::Invalid(msg)) => {
                state.writer.metrics().record_drop("collector_invalid");
                tracing::debug!(error = %msg, "collector event failed validation");
                continue;
            }
            Err(_) => continue,
        };
        match ingest(&state, event, Delivery::Enqueue).await {
            Ok(_) => {}
            Err(IngestError::Unavailable(_)) => break,
            Err(err) => tracing::debug!(?err, "collector event not stored"),
        }
    }
}
