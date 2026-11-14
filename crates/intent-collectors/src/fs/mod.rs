//! Filesystem watcher → `filesystem/workspace_seen`, `filesystem/file_modify`
//! and (opt-in) `filesystem/file_content`.
//!
//! * Approved roots arrive through a `tokio::sync::watch` channel; changes are
//!   applied live (1.x needed a service restart).
//! * Linux: one non-recursive inotify watch per directory, skipping ignored
//!   trees (`.git`, `node_modules`, `target`, ...) and symlinks, under a watch
//!   budget. Other platforms: one recursive watch per root.
//! * `/` is refused. The 1.x "all accessible" option maps to `$HOME` (hidden
//!   directories skipped) with its own smaller budget; hitting the budget or
//!   the kernel's `max_user_watches` is reported in the status detail.
//! * Debouncing is done by `notify-debouncer-full`; on top of that each path
//!   is reported at most once per `per_path_interval` (trailing edge).
//! * Content extraction runs off the event loop with bounded concurrency.

pub mod coalesce;
pub mod content;
pub mod filter;

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use intent_types::Payload;
use notify::{ErrorKind, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use serde_json::{Value, json};
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::time::Instant;

use crate::{Collector, CollectorContext, EventSink, SinkClosed, make_event};
use coalesce::{Coalescer, FsAction, classify_batch};
use content::Tools;
use filter::{is_ignored_dir_name, is_ignored_path, normalize_root, owning_root};

/// Caller-supplied sensitive-path check (`intent-capture` owns the rules).
pub type SensitivePredicate = Arc<dyn Fn(&Path) -> bool + Send + Sync>;

/// Linux uses per-directory watches so ignored trees cost nothing.
const PER_DIR: bool = cfg!(target_os = "linux");

/// What to watch. Send a new value on the `watch` channel to reconfigure.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatcherConfig {
    /// Approved workspace roots (`~` is expanded).
    pub workspaces: Vec<PathBuf>,
    /// 1.x "all accessible files" option: watches `$HOME` (never `/`).
    pub all_accessible: bool,
    /// Also emit `file_content` excerpts (detailed-capture consent).
    pub capture_content: bool,
}

#[derive(Debug, Clone)]
pub struct FsOptions {
    pub debounce: Duration,
    pub per_path_interval: Duration,
    pub max_file_bytes: u64,
    /// Total directory-watch budget (further capped at half the kernel limit).
    pub max_watches: usize,
    /// Budget for the `$HOME` (all-accessible) root.
    pub home_watch_budget: usize,
    pub content_concurrency: usize,
    /// Extractions queued beyond this are dropped (counted in status).
    pub content_queue_limit: usize,
    /// Defaults to `$HOME`.
    pub home: Option<PathBuf>,
}

impl Default for FsOptions {
    fn default() -> Self {
        Self {
            debounce: Duration::from_secs(2),
            per_path_interval: Duration::from_secs(10),
            max_file_bytes: filter::MAX_FILE_BYTES,
            max_watches: 65_536,
            home_watch_budget: 8_192,
            content_concurrency: 2,
            content_queue_limit: 64,
            home: std::env::var_os("HOME").map(PathBuf::from),
        }
    }
}

pub struct FilesystemCollector {
    pub config: watch::Receiver<WatcherConfig>,
    pub is_sensitive: SensitivePredicate,
    pub options: FsOptions,
}

impl FilesystemCollector {
    pub fn new(
        config: watch::Receiver<WatcherConfig>,
        is_sensitive: SensitivePredicate,
        options: FsOptions,
    ) -> Self {
        Self {
            config,
            is_sensitive,
            options,
        }
    }
}

impl Collector for FilesystemCollector {
    fn name(&self) -> &'static str {
        "filesystem"
    }

    async fn run(self, ctx: CollectorContext) {
        run_with(self, real_watcher, Tools::detect(), ctx).await
    }
}

// ---------------------------------------------------------------------------
// Watcher abstraction (real debouncer or a test fake)
// ---------------------------------------------------------------------------

pub(crate) trait DirWatcher: Send + 'static {
    fn watch_dir(&mut self, path: &Path, recursive: bool) -> notify::Result<()>;
    fn unwatch_dir(&mut self, path: &Path);
}

type RealDebouncer = Debouncer<notify::RecommendedWatcher, RecommendedCache>;

impl DirWatcher for RealDebouncer {
    fn watch_dir(&mut self, path: &Path, recursive: bool) -> notify::Result<()> {
        let mode = if recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        self.watch(path, mode)
    }
    fn unwatch_dir(&mut self, path: &Path) {
        let _ = self.unwatch(path);
    }
}

fn real_watcher(
    tx: mpsc::Sender<DebounceEventResult>,
    debounce: Duration,
) -> notify::Result<RealDebouncer> {
    // Runs on the debouncer's own thread (not a tokio worker), so blocking_send is fine
    // and gives natural back-pressure.
    new_debouncer(debounce, None, move |res: DebounceEventResult| {
        let _ = tx.blocking_send(res);
    })
}

fn backend_name() -> &'static str {
    if cfg!(target_os = "linux") {
        "inotify"
    } else if cfg!(target_os = "macos") {
        "fsevents"
    } else {
        "notify"
    }
}

/// `fs.inotify.max_user_watches`, if readable.
pub fn inotify_max_user_watches() -> Option<usize> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches")
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

fn has_hidden_component(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root)
        .map(|rel| {
            rel.components()
                .any(|c| is_hidden(&c.as_os_str().to_string_lossy()))
        })
        .unwrap_or(false)
}

/// Breadth-first list of directories to watch under `start` (inclusive),
/// skipping ignored names, symlinks, sensitive paths and (optionally) hidden
/// directories. Returns `(dirs, truncated_by_cap)`.
pub fn plan_watch_dirs(
    start: &Path,
    skip_hidden: bool,
    is_sensitive: &dyn Fn(&Path) -> bool,
    cap: usize,
) -> (Vec<PathBuf>, bool) {
    let mut out = Vec::new();
    let mut queue = VecDeque::from([start.to_path_buf()]);
    while let Some(dir) = queue.pop_front() {
        if out.len() >= cap {
            return (out, true);
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        out.push(dir);
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if !ft.is_dir() {
                continue; // includes symlinks: DirEntry::file_type does not follow them
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if is_ignored_dir_name(&name) || (skip_hidden && is_hidden(&name)) {
                continue;
            }
            let path = entry.path();
            if !is_sensitive(&path) {
                queue.push_back(path);
            }
        }
    }
    (out, false)
}

struct WatchSet<W> {
    watcher: W,
    watched: HashSet<PathBuf>,
    budget: usize,
    limit_hit: bool,
}

impl<W: DirWatcher> WatchSet<W> {
    /// Blocking: walks the tree and registers watches.
    fn add_tree(
        &mut self,
        start: &Path,
        skip_hidden: bool,
        cap: usize,
        is_sensitive: &dyn Fn(&Path) -> bool,
    ) {
        if !PER_DIR {
            if self.watched.insert(start.to_path_buf())
                && self.watcher.watch_dir(start, true).is_err()
            {
                self.watched.remove(start);
            }
            return;
        }
        let remaining = self.budget.saturating_sub(self.watched.len()).min(cap);
        let (dirs, truncated) = plan_watch_dirs(start, skip_hidden, is_sensitive, remaining);
        if truncated {
            self.limit_hit = true;
        }
        for d in dirs {
            if self.watched.contains(&d) {
                continue;
            }
            match self.watcher.watch_dir(&d, false) {
                Ok(()) => {
                    self.watched.insert(d);
                }
                Err(e) if matches!(e.kind, ErrorKind::MaxFilesWatch) => {
                    self.limit_hit = true;
                    break;
                }
                Err(_) => {} // permission denied, vanished, ...
            }
        }
    }

    fn remove_tree(&mut self, path: &Path) {
        let gone: Vec<PathBuf> = self
            .watched
            .iter()
            .filter(|p| p.starts_with(path))
            .cloned()
            .collect();
        for p in gone {
            self.watcher.unwatch_dir(&p);
            self.watched.remove(&p);
        }
    }
}

// ---------------------------------------------------------------------------
// Run loop
// ---------------------------------------------------------------------------

struct Roots {
    /// Explicit workspaces (emit `workspace_seen`).
    workspaces: Vec<PathBuf>,
    /// `$HOME` when all-accessible is on.
    home: Option<PathBuf>,
    /// All of the above, for owner lookup.
    all: Vec<PathBuf>,
}

fn resolve_roots(
    cfg: &WatcherConfig,
    opts: &FsOptions,
    is_sensitive: &dyn Fn(&Path) -> bool,
) -> (Roots, Vec<Value>) {
    let mut rejected = Vec::new();
    let mut workspaces = Vec::new();
    for raw in &cfg.workspaces {
        match normalize_root(raw, opts.home.as_deref(), is_sensitive) {
            Ok(p) if !workspaces.contains(&p) => workspaces.push(p),
            Ok(_) => {}
            Err(why) => {
                rejected.push(json!({"path": raw.to_string_lossy(), "reason": why.to_string()}))
            }
        }
    }
    let home = if cfg.all_accessible {
        match opts
            .home
            .as_deref()
            .map(|h| normalize_root(h, None, is_sensitive))
        {
            Some(Ok(h)) => Some(h),
            Some(Err(why)) => {
                rejected.push(json!({"path": "$HOME", "reason": why.to_string()}));
                None
            }
            None => {
                rejected.push(json!({"path": "$HOME", "reason": "HOME is not set"}));
                None
            }
        }
    } else {
        None
    };
    let mut all = workspaces.clone();
    if let Some(h) = &home
        && !all.contains(h)
    {
        all.push(h.clone());
    }
    (
        Roots {
            workspaces,
            home,
            all,
        },
        rejected,
    )
}

struct FsLoop<W> {
    sink: EventSink,
    ctx: CollectorContext,
    is_sensitive: SensitivePredicate,
    opts: FsOptions,
    tools: Arc<Tools>,
    capture_content: bool,
    roots: Roots,
    set: Option<Arc<Mutex<WatchSet<W>>>>,
    coalescer: Coalescer,
    sem: Arc<Semaphore>,
    inflight: Arc<AtomicUsize>,
    content_dropped: u64,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl<W: DirWatcher> FsLoop<W> {
    fn kernel_budget(&self) -> usize {
        match inotify_max_user_watches() {
            Some(k) => self.opts.max_watches.min(k / 2),
            None => self.opts.max_watches,
        }
    }

    fn publish_watch_detail(&self) {
        let (count, limit_hit, budget) = match &self.set {
            Some(s) => {
                let s = lock(s);
                (s.watched.len(), s.limit_hit, s.budget)
            }
            None => (0, false, 0),
        };
        let st = &self.ctx.status;
        st.set_detail("watched_dirs", count);
        st.set_detail("watch_budget", budget);
        st.set_detail("watch_limit_hit", limit_hit);
        st.set_detail(
            "inotify_max_user_watches",
            inotify_max_user_watches().map_or(Value::Null, Value::from),
        );
        if limit_hit {
            st.set_error(Some(format!(
                "watch budget reached ({count} directories); some folders are not watched. \
                 Narrow the approved workspaces or raise fs.inotify.max_user_watches"
            )));
        }
    }

    async fn apply<F>(
        &mut self,
        cfg: &WatcherConfig,
        factory: &F,
        btx: &mpsc::Sender<DebounceEventResult>,
    ) -> Result<(), SinkClosed>
    where
        F: Fn(mpsc::Sender<DebounceEventResult>, Duration) -> notify::Result<W>,
    {
        self.capture_content = cfg.capture_content;
        let pred = self.is_sensitive.clone();
        let (roots, rejected) = resolve_roots(cfg, &self.opts, &*pred);
        let previously: HashSet<PathBuf> = self.roots.workspaces.iter().cloned().collect();
        let st = &self.ctx.status;
        st.set_detail("rejected_roots", Value::Array(rejected));
        st.set_detail(
            "roots",
            roots
                .all
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        );
        st.set_detail("content_capture", cfg.capture_content);

        // Tear down the previous watcher before building the new one.
        self.set = None;
        if roots.all.is_empty() {
            self.roots = roots;
            st.set_waiting(
                Some(backend_name().into()),
                "no approved workspaces configured",
            );
            self.publish_watch_detail();
            return Ok(());
        }
        let watcher = match factory(btx.clone(), self.opts.debounce) {
            Ok(w) => w,
            Err(e) => {
                self.roots = roots;
                st.set_waiting(
                    Some(backend_name().into()),
                    format!("cannot create file watcher: {e}"),
                );
                return Ok(());
            }
        };
        let budget = self.kernel_budget();
        let home_budget = self.opts.home_watch_budget;
        let ws = roots.workspaces.clone();
        let home = roots.home.clone();
        let built = tokio::task::spawn_blocking(move || {
            let mut set = WatchSet {
                watcher,
                watched: HashSet::new(),
                budget,
                limit_hit: false,
            };
            for root in &ws {
                set.add_tree(root, false, usize::MAX, &*pred);
            }
            if let Some(h) = &home {
                set.add_tree(h, true, home_budget, &*pred);
            }
            set
        })
        .await;
        let set = match built {
            Ok(s) => s,
            Err(e) => {
                st.set_failed(format!("watch setup panicked: {e}"));
                return Ok(());
            }
        };
        self.set = Some(Arc::new(Mutex::new(set)));
        self.roots = roots;
        st.set_running(backend_name());
        self.publish_watch_detail();

        for root in self.roots.workspaces.clone() {
            if !previously.contains(&root) {
                let mut p = Payload::new();
                p.insert(
                    "workspace".into(),
                    Value::String(root.to_string_lossy().into_owned()),
                );
                self.sink.emit("filesystem", "workspace_seen", p).await?;
            }
        }
        Ok(())
    }

    fn root_of(&self, path: &Path) -> Option<(PathBuf, bool)> {
        let root = owning_root(path, &self.roots.all)?.clone();
        let home_mode =
            self.roots.home.as_ref() == Some(&root) && !self.roots.workspaces.contains(&root);
        Some((root, home_mode))
    }

    /// Eligible regular file → `(root)`; directories get watched as a side effect.
    async fn eligible(&mut self, path: &Path) -> Option<PathBuf> {
        let (root, home_mode) = self.root_of(path)?;
        if is_ignored_path(path, &root)
            || (home_mode && has_hidden_component(path, &root))
            || (self.is_sensitive)(path)
        {
            return None;
        }
        let meta = std::fs::symlink_metadata(path).ok()?;
        if meta.file_type().is_dir() {
            if PER_DIR && let Some(set) = &self.set {
                let already = lock(set).watched.contains(path);
                if !already {
                    let set = set.clone();
                    let pred = self.is_sensitive.clone();
                    let dir = path.to_path_buf();
                    let cap = if home_mode {
                        self.opts.home_watch_budget
                    } else {
                        usize::MAX
                    };
                    let _ = tokio::task::spawn_blocking(move || {
                        lock(&set).add_tree(&dir, home_mode, cap, &*pred)
                    })
                    .await;
                    self.publish_watch_detail();
                }
            }
            return None;
        }
        (meta.file_type().is_file() && meta.len() <= self.opts.max_file_bytes).then_some(root)
    }

    async fn emit_file(&mut self, path: PathBuf, root: &Path) -> Result<(), SinkClosed> {
        let workspace = root.to_string_lossy().into_owned();
        let mut p = Payload::new();
        p.insert(
            "path".into(),
            Value::String(path.to_string_lossy().into_owned()),
        );
        p.insert("workspace".into(), Value::String(workspace.clone()));
        self.sink.emit("filesystem", "file_modify", p).await?;

        if !self.capture_content {
            return Ok(());
        }
        if self.inflight.load(Ordering::Relaxed) >= self.opts.content_queue_limit {
            self.content_dropped += 1;
            self.ctx
                .status
                .set_detail("content_dropped", self.content_dropped);
            return Ok(());
        }
        self.inflight.fetch_add(1, Ordering::Relaxed);
        let (sem, inflight, tools, sink, cancel) = (
            self.sem.clone(),
            self.inflight.clone(),
            self.tools.clone(),
            self.sink.clone(),
            self.ctx.cancel.clone(),
        );
        let max = self.opts.max_file_bytes;
        tokio::spawn(async move {
            let work = async {
                let _permit = sem.acquire().await.ok()?;
                let mut payload = content::extract(path, max, &tools).await?;
                payload.insert("workspace".into(), Value::String(workspace));
                sink.send(make_event("filesystem", "file_content", payload))
                    .await
                    .ok()
            };
            tokio::select! {
                _ = cancel.cancelled() => {}
                _ = work => {}
            }
            inflight.fetch_sub(1, Ordering::Relaxed);
        });
        Ok(())
    }

    async fn handle(&mut self, res: DebounceEventResult) -> Result<(), SinkClosed> {
        match res {
            Ok(events) => {
                if events.iter().any(|e| e.event.need_rescan()) {
                    self.ctx.status.set_detail("rescan_needed", true);
                }
                for action in classify_batch(&events) {
                    match action {
                        FsAction::Touched(path) => {
                            if let Some(root) = self.eligible(&path).await
                                && self.coalescer.touch(path.clone(), Instant::now())
                            {
                                self.emit_file(path, &root).await?;
                            }
                        }
                        FsAction::Removed(path) => {
                            self.coalescer.forget(&path);
                            if PER_DIR && let Some(set) = &self.set {
                                lock(set).remove_tree(&path);
                            }
                        }
                    }
                }
            }
            Err(errors) => {
                for e in errors {
                    if matches!(e.kind, ErrorKind::MaxFilesWatch)
                        && let Some(set) = &self.set
                    {
                        lock(set).limit_hit = true;
                        self.publish_watch_detail();
                    } else {
                        self.ctx.status.set_error(Some(format!("watch error: {e}")));
                    }
                }
            }
        }
        Ok(())
    }

    async fn flush_due(&mut self) -> Result<(), SinkClosed> {
        for path in self.coalescer.due(Instant::now()) {
            if let Some(root) = self.eligible(&path).await {
                self.emit_file(path, &root).await?;
            }
        }
        Ok(())
    }
}

async fn sleep_until_opt(deadline: Option<Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
}

pub(crate) async fn run_with<W, F>(
    c: FilesystemCollector,
    factory: F,
    tools: Tools,
    ctx: CollectorContext,
) where
    W: DirWatcher,
    F: Fn(mpsc::Sender<DebounceEventResult>, Duration) -> notify::Result<W>,
{
    let FilesystemCollector {
        config: mut config_rx,
        is_sensitive,
        options,
    } = c;
    let (btx, mut brx) = mpsc::channel::<DebounceEventResult>(64);
    let mut lp = FsLoop::<W> {
        sink: ctx.sink.clone(),
        ctx: ctx.clone(),
        is_sensitive,
        sem: Arc::new(Semaphore::new(options.content_concurrency.max(1))),
        coalescer: Coalescer::new(options.per_path_interval),
        opts: options,
        tools: Arc::new(tools),
        capture_content: false,
        roots: Roots {
            workspaces: vec![],
            home: None,
            all: vec![],
        },
        set: None,
        inflight: Arc::new(AtomicUsize::new(0)),
        content_dropped: 0,
    };
    let mut cfg = config_rx.borrow_and_update().clone();
    if lp.apply(&cfg, &factory, &btx).await.is_err() {
        return;
    }
    let mut config_open = true;
    loop {
        let res = tokio::select! {
            _ = ctx.cancel.cancelled() => return,
            changed = config_rx.changed(), if config_open => {
                if changed.is_err() {
                    // Sender dropped: keep running with the last configuration.
                    config_open = false;
                    continue;
                }
                let next = config_rx.borrow_and_update().clone();
                if next == cfg {
                    continue;
                }
                let roots_changed = next.workspaces != cfg.workspaces || next.all_accessible != cfg.all_accessible;
                cfg = next;
                if roots_changed {
                    lp.apply(&cfg, &factory, &btx).await
                } else {
                    lp.capture_content = cfg.capture_content;
                    lp.ctx.status.set_detail("content_capture", cfg.capture_content);
                    Ok(())
                }
            }
            Some(batch) = brx.recv() => lp.handle(batch).await,
            _ = sleep_until_opt(lp.coalescer.next_deadline()) => lp.flush_due().await,
        };
        if res.is_err() {
            return; // sink closed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::{CollectorState, StatusHandle};
    use intent_types::RawEvent;
    use notify::event::{CreateKind, DataChange, ModifyKind};
    use notify::{Event, EventKind};
    use notify_debouncer_full::DebouncedEvent;

    #[test]
    fn plan_skips_ignored_hidden_and_respects_cap() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for d in [
            "src/a",
            "node_modules/x",
            ".git/objects",
            ".config/app",
            "secret/k",
            "docs",
        ] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("docs"), root.join("docs-link")).unwrap();
        let pred = |p: &Path| p.ends_with("secret");
        let (dirs, truncated) = plan_watch_dirs(&root, false, &pred, usize::MAX);
        assert!(!truncated);
        let rel: HashSet<String> = dirs
            .iter()
            .map(|d| {
                d.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let expect: HashSet<String> = ["", "src", "src/a", "docs", ".config", ".config/app"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(rel, expect);
        let (dirs, _) = plan_watch_dirs(&root, true, &pred, usize::MAX);
        assert!(
            dirs.iter()
                .all(|d| !d.to_string_lossy().contains(".config")),
            "hidden skipped in home mode"
        );
        let (dirs, truncated) = plan_watch_dirs(&root, false, &pred, 2);
        assert_eq!(dirs.len(), 2);
        assert!(truncated);
    }

    #[derive(Clone, Default)]
    struct Fake {
        watched: Arc<Mutex<Vec<PathBuf>>>,
        sender: Arc<Mutex<Option<mpsc::Sender<DebounceEventResult>>>>,
        builds: Arc<AtomicUsize>,
    }

    struct FakeWatcher(Arc<Mutex<Vec<PathBuf>>>);
    impl DirWatcher for FakeWatcher {
        fn watch_dir(&mut self, path: &Path, _recursive: bool) -> notify::Result<()> {
            lock(&self.0).push(path.to_path_buf());
            Ok(())
        }
        fn unwatch_dir(&mut self, path: &Path) {
            lock(&self.0).retain(|p| p != path);
        }
    }

    struct TestCollector(FilesystemCollector, Fake);
    impl Collector for TestCollector {
        fn name(&self) -> &'static str {
            "filesystem"
        }
        async fn run(self, ctx: CollectorContext) {
            let fake = self.1.clone();
            let factory = move |tx, _d| {
                fake.builds.fetch_add(1, Ordering::SeqCst);
                *lock(&fake.sender) = Some(tx);
                lock(&fake.watched).clear();
                Ok(FakeWatcher(fake.watched.clone()))
            };
            run_with(self.0, factory, Tools::default(), ctx).await
        }
    }

    fn batch(items: &[(EventKind, &Path)]) -> DebounceEventResult {
        Ok(items
            .iter()
            .map(|(k, p)| {
                DebouncedEvent::new(
                    Event::new(*k).add_path(p.to_path_buf()),
                    std::time::Instant::now(),
                )
            })
            .collect())
    }

    async fn next(rx: &mut mpsc::Receiver<RawEvent>) -> RawEvent {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("event in time")
            .unwrap()
    }

    async fn inject(fake: &Fake, b: DebounceEventResult) {
        let tx = lock(&fake.sender).clone().unwrap();
        tx.send(b).await.unwrap();
    }

    #[tokio::test]
    async fn fake_source_end_to_end_with_live_reconfigure() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let ws1 = base.join("ws1");
        let ws2 = base.join("ws2");
        std::fs::create_dir_all(ws1.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(&ws2).unwrap();
        std::fs::write(ws1.join("a.md"), "hello world").unwrap();
        std::fs::write(ws1.join("node_modules/pkg/i.js"), "x").unwrap();
        std::fs::write(ws1.join(".env"), "SECRET=1").unwrap();
        std::fs::write(ws2.join("b.txt"), "b").unwrap();

        let (cfg_tx, cfg_rx) = watch::channel(WatcherConfig {
            workspaces: vec![ws1.clone(), PathBuf::from("/")],
            all_accessible: false,
            capture_content: true,
        });
        let fake = Fake::default();
        let pred: SensitivePredicate =
            Arc::new(|p: &Path| p.file_name().is_some_and(|n| n == ".env"));
        let opts = FsOptions {
            per_path_interval: Duration::from_secs(60),
            home: None,
            ..Default::default()
        };
        let (tx, mut rx) = mpsc::channel(32);
        let cancel = crate::CancellationToken::new();
        let h = crate::spawn(
            TestCollector(FilesystemCollector::new(cfg_rx, pred, opts), fake.clone()),
            tx,
            cancel.clone(),
        );

        let seen = next(&mut rx).await;
        assert_eq!(
            (seen.source.as_str(), seen.kind.as_str()),
            ("filesystem", "workspace_seen")
        );
        assert_eq!(seen.payload["workspace"], ws1.to_string_lossy().as_ref());
        let st = h.status.snapshot();
        assert_eq!(st.state, CollectorState::Running);
        assert_eq!(st.detail["rejected_roots"][0]["path"], "/");
        if PER_DIR {
            assert!(
                !lock(&fake.watched)
                    .iter()
                    .any(|p| p.ends_with("node_modules"))
            );
        }

        let data = EventKind::Modify(ModifyKind::Data(DataChange::Content));
        inject(
            &fake,
            batch(&[
                (EventKind::Create(CreateKind::File), &ws1.join("a.md")),
                (data, &ws1.join("a.md")),
                (data, &ws1.join("node_modules/pkg/i.js")),
                (data, &ws1.join(".env")),
                (data, &ws2.join("b.txt")), // not approved yet
            ]),
        )
        .await;
        let m = next(&mut rx).await;
        assert_eq!(m.kind, "file_modify");
        assert_eq!(
            m.payload["path"],
            ws1.join("a.md").to_string_lossy().as_ref()
        );
        assert_eq!(m.payload["workspace"], ws1.to_string_lossy().as_ref());
        let c = next(&mut rx).await;
        assert_eq!(c.kind, "file_content");
        assert_eq!(c.payload["excerpt"], "hello world");
        assert_eq!(c.payload["workspace"], ws1.to_string_lossy().as_ref());

        // Second write inside the per-path window is coalesced.
        inject(&fake, batch(&[(data, &ws1.join("a.md"))])).await;

        // Hot reconfigure: add ws2 without restarting the collector.
        cfg_tx.send_modify(|c| {
            c.workspaces.push(ws2.clone());
            c.capture_content = false;
        });
        let seen2 = next(&mut rx).await;
        assert_eq!(seen2.kind, "workspace_seen");
        assert_eq!(seen2.payload["workspace"], ws2.to_string_lossy().as_ref());
        assert_eq!(fake.builds.load(Ordering::SeqCst), 2);
        inject(&fake, batch(&[(data, &ws2.join("b.txt"))])).await;
        let m2 = next(&mut rx).await;
        assert_eq!(m2.kind, "file_modify");
        assert_eq!(
            m2.payload["path"],
            ws2.join("b.txt").to_string_lossy().as_ref()
        );
        // No content event (capture turned off) and the coalesced a.md write produced nothing.
        assert!(
            tokio::time::timeout(Duration::from_millis(200), rx.recv())
                .await
                .is_err()
        );

        // Removing every root parks the collector in Waiting, not Failed.
        cfg_tx.send_modify(|c| c.workspaces.clear());
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(h.status.snapshot().state, CollectorState::Waiting);

        cancel.cancel();
        h.join.await.unwrap();
        assert_eq!(h.status.snapshot().state, CollectorState::Stopped);
    }

    #[tokio::test]
    async fn waiting_without_roots() {
        let (_cfg_tx, cfg_rx) = watch::channel(WatcherConfig::default());
        let (tx, _rx) = mpsc::channel(1);
        let cancel = crate::CancellationToken::new();
        let c = FilesystemCollector::new(cfg_rx, Arc::new(|_: &Path| false), FsOptions::default());
        let h = crate::spawn(TestCollector(c, Fake::default()), tx, cancel.clone());
        tokio::time::sleep(Duration::from_millis(50)).await;
        let st: crate::CollectorStatus = h.status.snapshot();
        assert_eq!(st.state, CollectorState::Waiting);
        assert!(st.running);
        cancel.cancel();
        h.join.await.unwrap();
        let _ = StatusHandle::new("x");
    }

    /// Real notify backend against a temp dir. Works on Linux (inotify) and macOS (FSEvents):
    /// `cargo test -p intent-collectors -- --ignored real_watcher_smoke`
    #[tokio::test]
    #[ignore = "touches the real OS file watcher; timing-dependent"]
    async fn real_watcher_smoke() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(ws.join("sub")).unwrap();
        let (_cfg_tx, cfg_rx) = watch::channel(WatcherConfig {
            workspaces: vec![ws.clone()],
            all_accessible: false,
            capture_content: false,
        });
        let opts = FsOptions {
            debounce: Duration::from_millis(200),
            ..Default::default()
        };
        let (tx, mut rx) = mpsc::channel(32);
        let cancel = crate::CancellationToken::new();
        let h = crate::spawn(
            FilesystemCollector::new(cfg_rx, Arc::new(|_: &Path| false), opts),
            tx,
            cancel.clone(),
        );
        assert_eq!(next(&mut rx).await.kind, "workspace_seen");
        tokio::time::sleep(Duration::from_millis(300)).await;
        std::fs::write(ws.join("sub/x.txt"), "1").unwrap();
        let ev = next(&mut rx).await;
        assert_eq!(ev.kind, "file_modify");
        assert!(ev.payload["path"].as_str().unwrap().ends_with("sub/x.txt"));
        // New directory created after start is picked up.
        std::fs::create_dir_all(ws.join("new")).unwrap();
        tokio::time::sleep(Duration::from_millis(800)).await;
        std::fs::write(ws.join("new/y.txt"), "2").unwrap();
        let ev = next(&mut rx).await;
        assert!(ev.payload["path"].as_str().unwrap().ends_with("new/y.txt"));
        cancel.cancel();
        h.join.await.unwrap();
    }
}
