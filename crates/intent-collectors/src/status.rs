//! Health / status reporting shared by every collector (surfaced by `/v1/status`).

use std::sync::{Arc, RwLock};

use serde::Serialize;
use serde_json::{Map, Value};

/// Lifecycle state of a collector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectorState {
    /// Spawned, still probing for a backend.
    Starting,
    /// A backend is attached and events are flowing (or would flow on activity).
    Running,
    /// No backend yet, but the collector is passively waiting for one to appear
    /// (e.g. the GNOME extension is installed but not enabled). Costs nothing.
    Waiting,
    /// No usable backend on this system. The collector task has exited cleanly.
    Unavailable,
    /// The collector hit an unrecoverable error (or panicked) and exited.
    Failed,
    /// Stopped by cancellation or because the event channel closed.
    Stopped,
}

/// Snapshot of a collector's health.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CollectorStatus {
    pub name: String,
    pub state: CollectorState,
    /// Convenience flag for UIs: `state` is `Running` or `Waiting`.
    pub running: bool,
    /// Backend in use, e.g. `x11`, `gnome-extension`, `mutter-idle-monitor`, `inotify`.
    pub backend: Option<String>,
    pub last_event_ts: Option<i64>,
    pub events_emitted: u64,
    pub error: Option<String>,
    /// Collector-specific extras (watch counts, idle_ms, ...).
    pub detail: Map<String, Value>,
}

impl CollectorStatus {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            state: CollectorState::Starting,
            running: false,
            backend: None,
            last_event_ts: None,
            events_emitted: 0,
            error: None,
            detail: Map::new(),
        }
    }
}

/// Cheap, cloneable handle to a collector's live status.
#[derive(Debug, Clone)]
pub struct StatusHandle(Arc<RwLock<CollectorStatus>>);

impl StatusHandle {
    pub fn new(name: impl Into<String>) -> Self {
        Self(Arc::new(RwLock::new(CollectorStatus::new(name))))
    }

    pub fn snapshot(&self) -> CollectorStatus {
        self.0.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn update(&self, f: impl FnOnce(&mut CollectorStatus)) {
        let mut guard = self.0.write().unwrap_or_else(|e| e.into_inner());
        f(&mut guard);
        guard.running = matches!(guard.state, CollectorState::Running | CollectorState::Waiting);
    }

    pub fn set_state(&self, state: CollectorState) {
        self.update(|s| s.state = state);
    }

    /// Mark the collector running on `backend` and clear any previous error.
    pub fn set_running(&self, backend: impl Into<String>) {
        let backend = backend.into();
        self.update(|s| {
            s.state = CollectorState::Running;
            s.backend = Some(backend);
            s.error = None;
        });
    }

    pub fn set_waiting(&self, backend: Option<String>, reason: impl Into<String>) {
        let reason = reason.into();
        self.update(|s| {
            s.state = CollectorState::Waiting;
            s.backend = backend;
            s.error = Some(reason);
        });
    }

    pub fn set_unavailable(&self, reason: impl Into<String>) {
        let reason = reason.into();
        self.update(|s| {
            s.state = CollectorState::Unavailable;
            s.error = Some(reason);
        });
    }

    pub fn set_failed(&self, reason: impl Into<String>) {
        let reason = reason.into();
        self.update(|s| {
            s.state = CollectorState::Failed;
            s.error = Some(reason);
        });
    }

    /// Record a non-fatal error without changing state.
    pub fn set_error(&self, reason: Option<String>) {
        self.update(|s| s.error = reason);
    }

    pub fn set_detail(&self, key: &str, value: impl Into<Value>) {
        let value = value.into();
        self.update(|s| {
            s.detail.insert(key.to_owned(), value);
        });
    }

    pub(crate) fn record_event(&self, ts: i64) {
        self.update(|s| {
            s.last_event_ts = Some(ts);
            s.events_emitted += 1;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_flag_tracks_state() {
        let h = StatusHandle::new("focus");
        assert!(!h.snapshot().running);
        h.set_running("x11");
        let s = h.snapshot();
        assert!(s.running);
        assert_eq!(s.backend.as_deref(), Some("x11"));
        h.set_unavailable("no display");
        let s = h.snapshot();
        assert!(!s.running);
        assert_eq!(s.state, CollectorState::Unavailable);
        assert_eq!(serde_json::to_value(&s).unwrap()["state"], "unavailable");
    }
}
