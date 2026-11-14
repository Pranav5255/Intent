//! Shared daemon state handed to every route and background task.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use intent_capture::restore::{LaunchCommand, SystemProbe};
use intent_capture::{ConfigStore, Paths};
use intent_collectors::{ActivityFeed, CancellationToken, StatusHandle, WatcherConfig};
use intent_engine::current::CurrentIntentEngine;
use intent_engine::prediction::PredictionEngine;
use intent_llm::settings::{LocalServerProbe, probe_local_server};
use intent_store::Store;
use tokio::sync::{Notify, watch};

use crate::auth::Token;
use crate::ingest::Writer;
use crate::llm::{LlmRuntime, ProviderFactory};
use crate::pause::Pause;

/// Launches one planned restore command; tests replace it so nothing is executed.
pub type RestoreSpawner = Arc<dyn Fn(&LaunchCommand) -> std::io::Result<()> + Send + Sync>;
/// Host probe used to plan restores; tests replace it.
pub type RestoreProbe = Arc<dyn SystemProbe + Send + Sync>;

pub struct Shared {
    pub paths: Paths,
    pub store: Store,
    pub config: Arc<ConfigStore>,
    pub token: Token,
    pub writer: Writer,
    pub feed: Arc<ActivityFeed>,
    pub pause: Pause,
    pub collectors: Mutex<Vec<StatusHandle>>,
    pub fs_watch: watch::Sender<WatcherConfig>,
    pub llm: ArcSwap<LlmRuntime>,
    pub llm_probe: Mutex<Option<(Instant, LocalServerProbe)>>,
    pub provider_factory: ProviderFactory,
    pub current: Arc<CurrentIntentEngine>,
    pub prediction: PredictionEngine,
    /// Serializes pipeline runs (scheduler vs. API).
    pub pipeline_lock: tokio::sync::Mutex<()>,
    pub restore_spawner: RestoreSpawner,
    pub restore_probe: Option<RestoreProbe>,
    /// Unix seconds of the first event inserted since the last scheduled run (0 = none).
    pub dirty_since: Arc<AtomicI64>,
    /// Wakes the embedding backfill.
    pub embed_wanted: Notify,
    pub started_at: i64,
    pub cancel: CancellationToken,
    pub enable_prediction: bool,
}

pub type AppState = Arc<Shared>;

const PROBE_TTL: Duration = Duration::from_secs(30);

impl Shared {
    pub fn llm(&self) -> Arc<LlmRuntime> {
        self.llm.load_full()
    }

    /// Rebuild providers from `settings` and swap them in atomically.
    pub fn install_llm(&self, settings: intent_llm::LlmSettings) {
        let runtime = LlmRuntime::build(settings, &self.provider_factory, &self.store, &self.current);
        self.llm.store(Arc::new(runtime));
        *self.llm_probe.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.current.clear();
        self.embed_wanted.notify_one();
    }

    /// Cached probe of the configured local server (`None` for cloud/none providers).
    pub async fn local_probe(&self, force: bool) -> Option<LocalServerProbe> {
        let runtime = self.llm();
        if !runtime.settings.is_local() {
            return None;
        }
        if !force
            && let Some((at, probe)) = self.llm_probe.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
            && at.elapsed() < PROBE_TTL
        {
            return Some(probe.clone());
        }
        let base = runtime.settings.effective_base_url()?;
        let probe = probe_local_server(&base, Duration::from_millis(1500)).await;
        *self.llm_probe.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), probe.clone()));
        Some(probe)
    }

    /// Whether inference calls are worth attempting right now.
    pub async fn inference_usable(&self) -> bool {
        let runtime = self.llm();
        if runtime.providers.harness.is_none() {
            return false;
        }
        match self.local_probe(false).await {
            Some(probe) => probe.reachable,
            None => true,
        }
    }

    pub fn mark_dirty(&self, ts: i64) {
        let _ = self.dirty_since.compare_exchange(0, ts.max(1), Ordering::Relaxed, Ordering::Relaxed);
    }

    /// Recompute what the filesystem collector should watch from the consent files and
    /// push it to the collector (applied live).
    pub fn refresh_watcher(&self) {
        let next = WatcherConfig {
            workspaces: self.config.approved_workspaces().into_iter().map(Into::into).collect(),
            all_accessible: self.config.filesystem_capture().map(|c| c.all_accessible).unwrap_or(false),
            capture_content: self.config.detailed_capture().map(|c| c.filesystem.enabled).unwrap_or(false),
        };
        self.fs_watch.send_if_modified(|current| {
            if *current == next {
                false
            } else {
                *current = next;
                true
            }
        });
    }
}
