//! Linux activity collectors for `intentd`.
//!
//! Every collector is an async tokio task that turns OS signals into
//! [`RawEvent`]s (same `source`/`type`/`payload` shapes as Intent 0.1) and pushes
//! them into a shared `mpsc` channel. Collectors share a [`CancellationToken`]
//! and publish a [`CollectorStatus`] for `/v1/status`.
//!
//! | Collector | Events | Backends |
//! |---|---|---|
//! | [`FocusCollector`] | `linux/app_focus` | X11 (`x11rb`, event-driven), GNOME Shell extension over D-Bus (Wayland + X11) |
//! | [`IdleCollector`] | `linux/idle_start`, `linux/idle_end` | `org.gnome.Mutter.IdleMonitor` (D-Bus watches), X11 MIT-SCREEN-SAVER |
//! | [`FilesystemCollector`] | `filesystem/workspace_seen`, `filesystem/file_modify`, `filesystem/file_content` | `notify` + `notify-debouncer-full` (inotify on Linux) |
//!
//! When no backend is usable a collector sets its state to
//! [`CollectorState::Unavailable`] and returns — it never crash-loops.
//! Linux-only backends are behind `cfg(target_os = "linux")`; on other targets
//! focus/idle report `Unavailable` immediately.
//!
//! ## Wiring (in `intentd`)
//!
//! ```no_run
//! use std::{path::Path, sync::Arc};
//! use intent_collectors::*;
//! use tokio::sync::{mpsc, watch};
//!
//! # async fn wire(is_sensitive_path: fn(&Path) -> bool) {
//! let (tx, mut rx) = mpsc::channel(1024);
//! let cancel = CancellationToken::new();
//! let feed = Arc::new(ActivityFeed::new());
//! let (fs_cfg_tx, fs_cfg_rx) = watch::channel(WatcherConfig::default());
//!
//! let handles = vec![
//!     spawn(FocusCollector::new(FocusConfig::default(), Some(feed.clone())), tx.clone(), cancel.clone()),
//!     spawn(IdleCollector::new(IdleConfig::default(), feed.clone()), tx.clone(), cancel.clone()),
//!     spawn(
//!         FilesystemCollector::new(fs_cfg_rx, Arc::new(is_sensitive_path), FsOptions::default()),
//!         tx.clone(),
//!         cancel.clone(),
//!     ),
//! ];
//! // UI changed the approved workspaces: applied live, no restart.
//! fs_cfg_tx.send_modify(|c| c.workspaces.push("/home/me/src/project".into()));
//! // /v1/status: handles.iter().map(|h| h.status()) and feed.snapshot().
//! while let Some(event) = rx.recv().await { /* validate + store */ let _ = event; }
//! cancel.cancel();
//! for h in handles { let _ = h.join.await; }
//! # }
//! ```

use std::future::Future;

use intent_types::{Payload, RawEvent};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
pub use tokio_util::sync::CancellationToken;

pub mod feed;
pub mod focus;
pub mod fs;
pub mod idle;
pub mod session;
pub mod status;

pub use feed::{ActivityFeed, ActivityKind, FeedSnapshot};
pub use focus::{FocusBackendPref, FocusCollector, FocusConfig};
pub use fs::{FilesystemCollector, FsOptions, SensitivePredicate, WatcherConfig};
pub use idle::{IdleBackendPref, IdleCollector, IdleConfig};
pub use status::{CollectorState, CollectorStatus, StatusHandle};

/// Current wall-clock time in unix seconds.
pub fn now_ts() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Build a `RawEvent` with a fresh uuid v4 id and the current unix-second timestamp.
pub fn make_event(source: &str, kind: &str, payload: Payload) -> RawEvent {
    RawEvent {
        id: uuid::Uuid::new_v4().to_string(),
        ts: now_ts(),
        source: source.to_owned(),
        kind: kind.to_owned(),
        payload,
        schema_version: 1,
        ingested_at: None,
    }
}

/// The receiving side of the event channel has gone away (daemon shutting down).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkClosed;

impl std::fmt::Display for SinkClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("event channel closed")
    }
}

impl std::error::Error for SinkClosed {}

/// Sending half handed to a collector: forwards events and keeps
/// `last_event_ts` / `events_emitted` up to date.
#[derive(Debug, Clone)]
pub struct EventSink {
    tx: mpsc::Sender<RawEvent>,
    status: StatusHandle,
}

impl EventSink {
    pub fn new(tx: mpsc::Sender<RawEvent>, status: StatusHandle) -> Self {
        Self { tx, status }
    }

    pub async fn send(&self, event: RawEvent) -> Result<(), SinkClosed> {
        let ts = event.ts;
        self.tx.send(event).await.map_err(|_| SinkClosed)?;
        self.status.record_event(ts);
        Ok(())
    }

    /// Convenience: build and send an event.
    pub async fn emit(&self, source: &str, kind: &str, payload: Payload) -> Result<(), SinkClosed> {
        self.send(make_event(source, kind, payload)).await
    }
}

/// Everything a collector needs at run time.
#[derive(Debug, Clone)]
pub struct CollectorContext {
    pub sink: EventSink,
    pub status: StatusHandle,
    pub cancel: CancellationToken,
}

/// An activity source. `run` must return when `ctx.cancel` fires or when the
/// sink is closed, and must set an appropriate final state on `ctx.status`
/// (`Unavailable` / `Failed`); anything still `Running`/`Waiting`/`Starting`
/// when `run` returns is reported as `Stopped`.
pub trait Collector: Send + 'static {
    /// Stable name used in `/v1/status` (`focus`, `idle`, `filesystem`).
    fn name(&self) -> &'static str;
    fn run(self, ctx: CollectorContext) -> impl Future<Output = ()> + Send;
}

/// A spawned collector.
#[derive(Debug)]
pub struct CollectorHandle {
    pub name: &'static str,
    pub status: StatusHandle,
    pub join: JoinHandle<()>,
}

impl CollectorHandle {
    pub fn status(&self) -> CollectorStatus {
        self.status.snapshot()
    }
}

/// Spawn `collector` on the current tokio runtime. Panics inside the collector
/// are caught and reported as `Failed`; they never take down the daemon.
pub fn spawn<C: Collector>(collector: C, tx: mpsc::Sender<RawEvent>, cancel: CancellationToken) -> CollectorHandle {
    let name = collector.name();
    let status = StatusHandle::new(name);
    let ctx = CollectorContext { sink: EventSink::new(tx, status.clone()), status: status.clone(), cancel };
    let inner = tokio::spawn(collector.run(ctx));
    let st = status.clone();
    let join = tokio::spawn(async move {
        match inner.await {
            Ok(()) => st.update(|s| {
                if matches!(s.state, CollectorState::Running | CollectorState::Waiting | CollectorState::Starting) {
                    s.state = CollectorState::Stopped;
                }
            }),
            Err(err) if err.is_panic() => {
                tracing::error!(collector = name, "collector panicked");
                st.set_failed("collector panicked");
            }
            Err(_) => st.set_state(CollectorState::Stopped),
        }
    });
    CollectorHandle { name, status, join }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Boom;
    impl Collector for Boom {
        fn name(&self) -> &'static str {
            "boom"
        }
        async fn run(self, ctx: CollectorContext) {
            ctx.status.set_running("test");
            ctx.sink.emit("linux", "idle_start", Payload::new()).await.unwrap();
            panic!("boom");
        }
    }

    struct Quiet;
    impl Collector for Quiet {
        fn name(&self) -> &'static str {
            "quiet"
        }
        async fn run(self, ctx: CollectorContext) {
            ctx.status.set_running("test");
            ctx.cancel.cancelled().await;
        }
    }

    #[tokio::test]
    async fn panics_become_failed_status() {
        let (tx, mut rx) = mpsc::channel(4);
        let h = spawn(Boom, tx, CancellationToken::new());
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.source, "linux");
        assert_eq!(ev.kind, "idle_start");
        assert_eq!(uuid::Uuid::parse_str(&ev.id).unwrap().get_version_num(), 4);
        h.join.await.unwrap();
        let s = h.status.snapshot();
        assert_eq!(s.state, CollectorState::Failed);
        assert_eq!(s.events_emitted, 1);
        assert!(s.last_event_ts.is_some());
    }

    #[tokio::test]
    async fn cancellation_stops() {
        let (tx, _rx) = mpsc::channel(4);
        let cancel = CancellationToken::new();
        let h = spawn(Quiet, tx, cancel.clone());
        tokio::task::yield_now().await;
        cancel.cancel();
        h.join.await.unwrap();
        assert_eq!(h.status.snapshot().state, CollectorState::Stopped);
    }
}
