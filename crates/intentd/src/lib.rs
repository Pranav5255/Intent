//! `intentd` — the Intent activity daemon.
//!
//! One process replaces the 1.x capture server, engine API, scheduled-ingest timer,
//! X11 tracker and workspace watcher: an axum server on 127.0.0.1:9477, a batched
//! event writer, in-process collectors, and a few timer-driven background jobs, all
//! over one SQLite database (`intent-store`).
//!
//! The binary (`main.rs`) only adds logging, signals and sd_notify; tests drive
//! [`Daemon`] directly on an ephemeral port.

pub mod auth;
mod collectors;
pub mod error;
pub mod ingest;
pub mod jobs;
pub mod llm;
#[cfg(target_os = "linux")]
mod notify;
pub mod pause;
pub mod pipeline;
pub mod routes;
pub mod state;
pub mod util;

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use arc_swap::ArcSwap;
use axum::Router;
use intent_capture::restore::spawn_detached;
use intent_capture::{ConfigStore, Paths};
use intent_collectors::{ActivityFeed, ActivityKind, CancellationToken, WatcherConfig};
use intent_engine::current::CurrentIntentEngine;
use intent_engine::prediction::PredictionEngine;
use intent_llm::LlmSettings;
use intent_store::{Store, StoreOptions, now_ts};
use intent_types::RawEvent;
use tokio::net::TcpListener;
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;

pub use crate::ingest::WriterConfig;
pub use crate::llm::{LlmRuntime, ProviderFactory, default_factory};
pub use crate::state::{AppState, RestoreProbe, RestoreSpawner, Shared};

use crate::auth::Token;
use crate::ingest::{InsertHook, Writer};

/// Everything configurable about a daemon instance.
#[derive(Clone)]
pub struct DaemonOptions {
    pub paths: Paths,
    pub bind: SocketAddr,
    /// Spawn focus/idle/filesystem collectors.
    pub collectors: bool,
    /// Spawn the scheduler, retention, pause-expiry, notification and embedding jobs.
    pub jobs: bool,
    pub writer: WriterConfig,
    pub provider_factory: ProviderFactory,
    pub restore_spawner: RestoreSpawner,
    pub restore_probe: Option<RestoreProbe>,
    pub firefox_origin: Option<String>,
    pub read_connections: usize,
    pub enable_prediction: bool,
}

impl DaemonOptions {
    /// Production defaults for `paths`.
    pub fn new(paths: Paths) -> Self {
        Self {
            paths,
            bind: SocketAddr::from(([127, 0, 0, 1], intent_types::DEFAULT_PORT)),
            collectors: true,
            jobs: true,
            writer: WriterConfig::default(),
            provider_factory: default_factory(),
            restore_spawner: Arc::new(spawn_detached),
            restore_probe: None,
            firefox_origin: None,
            read_connections: 2,
            enable_prediction: true,
        }
    }

    /// Defaults plus the process environment (`INTENT_BIND`, `INTENT_FIREFOX_EXTENSION_ORIGIN`,
    /// `INTENT_INGEST_*`, `INTENT_ENABLE_PREDICTION`, `INTENT_DISABLE_COLLECTORS`).
    pub fn from_env() -> anyhow::Result<Self> {
        let mut opts = Self::new(Paths::from_env());
        if let Ok(bind) = std::env::var("INTENT_BIND") {
            opts.bind = bind.trim().parse().with_context(|| format!("INTENT_BIND={bind:?} is not host:port"))?;
            if !opts.bind.ip().is_loopback() {
                anyhow::bail!("INTENT_BIND must be a loopback address");
            }
        }
        opts.firefox_origin = std::env::var("INTENT_FIREFOX_EXTENSION_ORIGIN").ok();
        opts.writer = WriterConfig::from_env();
        let off = |key: &str| std::env::var(key).is_ok_and(|v| matches!(v.trim(), "0" | "false" | "no"));
        let on = |key: &str| std::env::var(key).is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes"));
        opts.enable_prediction = !off("INTENT_ENABLE_PREDICTION");
        opts.collectors = !on("INTENT_DISABLE_COLLECTORS");
        Ok(opts)
    }
}

/// A running daemon (router not yet served).
pub struct Daemon {
    pub state: AppState,
    pub router: Router,
    pub token_path: PathBuf,
    collectors: Option<collectors::Spawned>,
    jobs: Vec<JoinHandle<()>>,
}

fn record_activity(feed: &ActivityFeed, event: &RawEvent) {
    let kind = match (event.source.as_str(), event.kind.as_str()) {
        ("linux", "app_focus") => ActivityKind::Focus,
        ("shell", "command") => ActivityKind::Shell,
        ("vscode", "document_change") => ActivityKind::Key,
        _ => return,
    };
    feed.record(kind);
}

impl Daemon {
    pub async fn start(opts: DaemonOptions) -> anyhow::Result<Daemon> {
        let paths = opts.paths.clone();
        let data_dir = paths.data_dir();
        let db_path = paths.database();
        let read_connections = opts.read_connections;
        let store = tokio::task::spawn_blocking(move || -> anyhow::Result<Store> {
            std::fs::create_dir_all(&data_dir).with_context(|| format!("creating {}", data_dir.display()))?;
            let store = Store::open(&db_path, StoreOptions { read_connections, ..Default::default() })
                .with_context(|| format!("opening {}", db_path.display()))?;
            Ok(store)
        })
        .await??;
        import_legacy(&store, &paths).await;

        let token_paths = vec![paths.token(), paths.data_dir().join(intent_capture::paths::TOKEN_FILE)];
        let (token, token_path) = Token::load_or_create(&token_paths).context("creating the API token")?;

        let config = Arc::new(ConfigStore::new(&paths));
        let settings = match LlmSettings::load_or_import(&paths.config_dir()) {
            Ok((settings, imported)) => {
                if imported {
                    tracing::info!("imported 1.x LLM settings");
                }
                settings
            }
            Err(err) => {
                tracing::warn!(error = %err, "LLM settings unreadable; using local-first defaults");
                LlmSettings::default()
            }
        };

        let feed = Arc::new(ActivityFeed::new());
        let dirty = Arc::new(AtomicI64::new(0));
        let hook: InsertHook = {
            let feed = Arc::clone(&feed);
            let dirty = Arc::clone(&dirty);
            Arc::new(move |inserted: &[&RawEvent]| {
                for event in inserted {
                    record_activity(&feed, event);
                }
                let _ = dirty.compare_exchange(0, now_ts().max(1), Ordering::Relaxed, Ordering::Relaxed);
            })
        };
        let writer = Writer::spawn(store.clone(), opts.writer.clone(), hook);
        let current = Arc::new(CurrentIntentEngine::default());
        let runtime = LlmRuntime::build(settings, &opts.provider_factory, &store, &current);
        if let Some(spec) = runtime.embedder_spec() {
            let s = store.clone();
            match tokio::task::spawn_blocking(move || s.configure_embeddings(&spec)).await? {
                Ok(true) => tracing::info!("embedder changed; intent vectors will be rebuilt"),
                Ok(false) => {}
                Err(err) => tracing::warn!(error = %err, "could not configure embeddings"),
            }
        }
        let pause = {
            let s = store.clone();
            tokio::task::spawn_blocking(move || pause::Pause::load(&s)).await?
        };
        if pause.is_paused() {
            tracing::info!(until = ?pause.current().until, "capture is paused (persisted)");
        }
        let (fs_watch, _) = watch::channel(WatcherConfig::default());

        let state: AppState = Arc::new(Shared {
            paths,
            store,
            config,
            token,
            writer,
            feed,
            pause,
            collectors: Mutex::new(Vec::new()),
            fs_watch,
            llm: ArcSwap::from_pointee(runtime),
            llm_probe: Mutex::new(None),
            provider_factory: opts.provider_factory.clone(),
            current,
            prediction: PredictionEngine::new(),
            pipeline_lock: tokio::sync::Mutex::new(()),
            restore_spawner: opts.restore_spawner.clone(),
            restore_probe: opts.restore_probe.clone(),
            dirty_since: dirty,
            embed_wanted: Notify::new(),
            started_at: now_ts(),
            cancel: CancellationToken::new(),
            enable_prediction: opts.enable_prediction,
        });
        state.refresh_watcher();

        let collectors = opts.collectors.then(|| collectors::start(&state));
        let jobs = if opts.jobs { jobs::spawn(&state) } else { Vec::new() };
        let router = routes::router(Arc::clone(&state), routes::cors_layer(opts.firefox_origin.as_deref()));
        Ok(Daemon { state, router, token_path, collectors, jobs })
    }

    /// The filesystem collector's current configuration (test hook / diagnostics).
    pub fn watcher_config(&self) -> WatcherConfig {
        self.state.fs_watch.borrow().clone()
    }

    pub fn token(&self) -> &str {
        self.state.token.as_str()
    }

    /// Serve until `shutdown` resolves, then stop collectors/jobs and flush the writer.
    pub async fn serve(self, listener: TcpListener, shutdown: impl Future<Output = ()> + Send + 'static) -> anyhow::Result<()> {
        let router = self.router.clone();
        axum::serve(listener, router).with_graceful_shutdown(shutdown).await.context("HTTP server failed")?;
        self.shutdown().await;
        Ok(())
    }

    /// Cancel collectors and jobs, drain the event queue and checkpoint the database.
    pub async fn shutdown(self) {
        let state = self.state;
        state.cancel.cancel();
        if let Some(spawned) = self.collectors {
            for handle in spawned.handles {
                let _ = tokio::time::timeout(std::time::Duration::from_secs(2), handle.join).await;
            }
            // The forwarder ends once every collector dropped its sender.
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), spawned.forwarder).await;
        }
        state.writer.close();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), state.writer.flush()).await;
        for job in self.jobs {
            job.abort();
        }
        let store = state.store.clone();
        let _ = tokio::task::spawn_blocking(move || store.maintenance()).await;
        tracing::info!("intentd stopped");
    }
}

/// One-time import of 1.x `events.db` / `intents.db` (no-op once flagged).
async fn import_legacy(store: &Store, paths: &Paths) {
    let events_db = paths.legacy_events_db();
    let intents_db = paths.data_dir().join("intents.db");
    if !events_db.exists() && !intents_db.exists() {
        return;
    }
    let s = store.clone();
    match tokio::task::spawn_blocking(move || s.import_legacy(&events_db, &intents_db)).await {
        Ok(Ok(report)) => {
            if report.events_imported > 0 || report.intents_imported > 0 {
                tracing::info!(
                    events = report.events_imported,
                    intents = report.intents_imported,
                    runs = report.runs_imported,
                    "imported 1.x data"
                );
            }
        }
        Ok(Err(err)) => tracing::warn!(error = %err, "1.x data import failed; continuing without it"),
        Err(err) => tracing::warn!(error = %err, "1.x data import panicked"),
    }
}
