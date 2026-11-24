//! Filesystem watcher → `filesystem/workspace_seen`, `filesystem/file_modify`
//! and (opt-in) `filesystem/file_content`.
//!
//! * Approved roots arrive through a `tokio::sync::watch` channel; changes are
//!   applied live (0.1 needed a service restart).
//! * Linux: one non-recursive inotify watch per directory, skipping ignored
//!   trees (`.git`, `node_modules`, `target`, ...) and symlinks, under a watch
//!   budget. Other platforms: one recursive watch per root.
//! * A root that is deleted or renamed away (or does not exist yet) is kept: its
//!   nearest existing parent is watched (non-recursively) until the root comes
//!   back, and then its whole tree is watched again. Missing roots are listed in
//!   the status detail as `missing_roots`.
//! * `/` is refused. The 0.1 "all accessible" option maps to `$HOME` (hidden
//!   directories skipped) with its own smaller budget; hitting the budget or
//!   the kernel's `max_user_watches` is reported in the status detail.
//! * Debouncing is done by `notify-debouncer-full`; on top of that each path
//!   is reported at most once per `per_path_interval` (trailing edge).
//! * Content extraction runs off the event loop with bounded concurrency.

pub mod coalesce;
pub mod content;
pub mod filter;

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use intent_types::Payload;
use notify::{ErrorKind, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use serde_json::{Value, json};
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::time::Instant;

use crate::{Collector, CollectorContext, EventSink, SinkClosed, make_event};
use coalesce::{Coalescer, FsAction, classify_batch};
use content::Tools;
use filter::{
    RootRejection, is_ignored_dir_name, is_ignored_path, missing_root, nearest_existing_parent, normalize_root,
    owning_root,
};

/// Caller-supplied sensitive-path check (`intent-capture` owns the rules).
pub type SensitivePredicate = Arc<dyn Fn(&Path) -> bool + Send + Sync>;

/// Linux uses per-directory watches so ignored trees cost nothing.
const PER_DIR: bool = cfg!(target_os = "linux");

/// What to watch. Send a new value on the `watch` channel to reconfigure.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatcherConfig {
    /// Approved workspace roots (`~` is expanded).
    pub workspaces: Vec<PathBuf>,
    /// 0.1 "all accessible files" option: watches `$HOME` (never `/`).
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
    /// Folders never watched or reported, with everything below them (Intent's own
    /// data, config and runtime dirs, so its writes never feed back into capture).
    pub never_watch: Vec<PathBuf>,
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
            never_watch: Vec::new(),
        }
    }
}

pub struct FilesystemCollector {
    pub config: watch::Receiver<WatcherConfig>,
    pub is_sensitive: SensitivePredicate,
    pub options: FsOptions,
}

impl FilesystemCollector {
    pub fn new(config: watch::Receiver<WatcherConfig>, is_sensitive: SensitivePredicate, options: FsOptions) -> Self {
        Self { config, is_sensitive, options }
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
        let mode = if recursive { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
        self.watch(path, mode)
    }
    fn unwatch_dir(&mut self, path: &Path) {
        let _ = self.unwatch(path);
    }
}

fn real_watcher(tx: mpsc::Sender<DebounceEventResult>, debounce: Duration) -> notify::Result<RealDebouncer> {
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
    std::fs::read_to_string("/proc/sys/fs/inotify/max_user_watches").ok()?.trim().parse().ok()
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

fn has_hidden_component(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root)
        .map(|rel| rel.components().any(|c| is_hidden(&c.as_os_str().to_string_lossy())))
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
    /// Non-recursive watches on the nearest existing parent of each missing root.
    parents: HashSet<PathBuf>,
    budget: usize,
    limit_hit: bool,
}

impl<W: DirWatcher> WatchSet<W> {
    /// Blocking: walks the tree and registers watches.
    fn add_tree(&mut self, start: &Path, skip_hidden: bool, cap: usize, is_sensitive: &dyn Fn(&Path) -> bool) {
        if !PER_DIR {
            if self.watched.insert(start.to_path_buf()) && self.watcher.watch_dir(start, true).is_err() {
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
        let gone: Vec<PathBuf> = self.watched.iter().filter(|p| p.starts_with(path)).cloned().collect();
        for p in gone {
            self.watcher.unwatch_dir(&p);
            self.watched.remove(&p);
            // A parent watch on the same directory is gone with it.
            self.parents.remove(&p);
        }
        self.parents.retain(|p| !p.starts_with(path));
    }

    /// Watch exactly `wanted` as parent directories. A directory that is also part
    /// of a watched tree shares that watch and is never unwatched here.
    fn set_parents(&mut self, wanted: &HashSet<PathBuf>) {
        let stale: Vec<PathBuf> = self.parents.difference(wanted).cloned().collect();
        for p in stale {
            if !self.watched.contains(&p) {
                self.watcher.unwatch_dir(&p);
            }
            self.parents.remove(&p);
        }
        for p in wanted {
            if self.parents.contains(p) {
                continue;
            }
            if self.watched.contains(p) || self.watcher.watch_dir(p, false).is_ok() {
                self.parents.insert(p.clone());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Run loop
// ---------------------------------------------------------------------------

struct Roots {
    /// Explicit workspaces (emit `workspace_seen`), including missing ones.
    workspaces: Vec<PathBuf>,
    /// `$HOME` when all-accessible is on.
    home: Option<PathBuf>,
    /// All of the above, for owner lookup.
    all: Vec<PathBuf>,
    /// Workspaces that did not exist when the configuration was applied.
    missing: Vec<PathBuf>,
}

fn resolve_roots(cfg: &WatcherConfig, opts: &FsOptions, is_sensitive: &dyn Fn(&Path) -> bool) -> (Roots, Vec<Value>) {
    let mut rejected = Vec::new();
    let mut workspaces = Vec::new();
    let mut missing = Vec::new();
    for raw in &cfg.workspaces {
        // A missing root is kept and watched for (no `if let` guard: MSRV is 1.91).
        let root = match normalize_root(raw, opts.home.as_deref(), is_sensitive) {
            Ok(p) => p,
            Err(why) => {
                let missing_path = matches!(why, RootRejection::NotADirectory)
                    .then(|| missing_root(raw, opts.home.as_deref(), is_sensitive))
                    .flatten();
                let Some(p) = missing_path else {
                    rejected.push(json!({"path": raw.to_string_lossy(), "reason": why.to_string()}));
                    continue;
                };
                if !missing.contains(&p) {
                    missing.push(p.clone());
                }
                p
            }
        };
        if !workspaces.contains(&root) {
            workspaces.push(root);
        }
    }
    let home = if cfg.all_accessible {
        match opts.home.as_deref().map(|h| normalize_root(h, None, is_sensitive)) {
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
    (Roots { workspaces, home, all, missing }, rejected)
}

struct FsLoop<W> {
    sink: EventSink,
    ctx: CollectorContext,
    is_sensitive: SensitivePredicate,
    opts: FsOptions,
    tools: Arc<Tools>,
    capture_content: bool,
    roots: Roots,
    /// Roots that do not exist right now, with when they went missing; their
    /// parents are watched instead.
    missing: BTreeMap<PathBuf, SystemTime>,
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
        st.set_detail("inotify_max_user_watches", inotify_max_user_watches().map_or(Value::Null, Value::from));
        let missing: Vec<String> = self.missing.keys().map(|p| p.to_string_lossy().into_owned()).collect();
        let was_missing =
            st.snapshot().detail.get("missing_roots").and_then(Value::as_array).is_some_and(|a| !a.is_empty());
        if limit_hit {
            st.set_error(Some(format!(
                "watch budget reached ({count} directories); some folders are not watched. \
                 Narrow the approved workspaces or raise fs.inotify.max_user_watches"
            )));
        } else if !missing.is_empty() {
            st.set_error(Some(format!(
                "workspace folder missing: {}; capture resumes when it is back",
                missing.join(", ")
            )));
        } else if was_missing {
            st.set_error(None);
        }
        st.set_detail("missing_roots", missing);
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
        st.set_detail("roots", roots.all.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>());
        st.set_detail("content_capture", cfg.capture_content);

        // Tear down the previous watcher before building the new one.
        self.set = None;
        let now = SystemTime::now();
        self.missing = roots.missing.iter().map(|root| (root.clone(), now)).collect();
        if roots.all.is_empty() {
            self.roots = roots;
            st.set_waiting(Some(backend_name().into()), "no approved workspaces configured");
            self.publish_watch_detail();
            return Ok(());
        }
        let watcher = match factory(btx.clone(), self.opts.debounce) {
            Ok(w) => w,
            Err(e) => {
                self.roots = roots;
                st.set_waiting(Some(backend_name().into()), format!("cannot create file watcher: {e}"));
                return Ok(());
            }
        };
        let budget = self.kernel_budget();
        let home_budget = self.opts.home_watch_budget;
        let ws: Vec<PathBuf> = roots.workspaces.iter().filter(|r| !self.missing.contains_key(*r)).cloned().collect();
        let home = roots.home.clone();
        let built = tokio::task::spawn_blocking(move || {
            let mut set =
                WatchSet { watcher, watched: HashSet::new(), parents: HashSet::new(), budget, limit_hit: false };
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
            if !previously.contains(&root) && !self.missing.contains_key(&root) {
                self.emit_workspace_seen(&root).await?;
            }
        }
        self.recheck_missing().await
    }

    async fn emit_workspace_seen(&self, root: &Path) -> Result<(), SinkClosed> {
        let mut p = Payload::new();
        p.insert("workspace".into(), Value::String(root.to_string_lossy().into_owned()));
        self.sink.emit("filesystem", "workspace_seen", p).await
    }

    /// Roots at or below `path`, which was just removed or renamed away, are missing
    /// now (`recheck_missing` re-adds any that already came back).
    fn mark_missing_under(&mut self, path: &Path) -> bool {
        let gone: Vec<PathBuf> = self.roots.all.iter().filter(|r| r.starts_with(path)).cloned().collect();
        let any = !gone.is_empty();
        let now = SystemTime::now();
        for root in gone {
            self.missing.entry(root).or_insert(now);
        }
        any
    }

    /// Whether a change to `path` can bring a missing root back: `path` is the root
    /// or one of its parents.
    fn affects_missing(&self, path: &Path) -> bool {
        self.missing.keys().any(|root| root.starts_with(path))
    }

    /// Watch every missing root that exists again (whole tree, then `workspace_seen`),
    /// and the nearest existing parent of each root that is still missing.
    ///
    /// A root is often back before its removal is even reported (`rm -rf src &&
    /// git clone ... src`), so files written in it since it went missing are
    /// reported too, up to [`RECOVERED_FILES_LIMIT`] per root.
    async fn recheck_missing(&mut self) -> Result<(), SinkClosed> {
        let Some(set) = self.set.clone() else {
            return Ok(());
        };
        let back: Vec<(PathBuf, SystemTime)> =
            self.missing.iter().filter(|(root, _)| root.is_dir()).map(|(r, t)| (r.clone(), *t)).collect();
        for (root, _) in &back {
            self.missing.remove(root);
        }
        let wanted: HashSet<PathBuf> = self.missing.keys().filter_map(|root| nearest_existing_parent(root)).collect();
        let pred = self.is_sensitive.clone();
        let home = self.roots.home.clone();
        let home_budget = self.opts.home_watch_budget;
        let trees = back.clone();
        let done = tokio::task::spawn_blocking(move || {
            let mut set = lock(&set);
            let mut written = Vec::new();
            for (root, since) in &trees {
                let home_mode = home.as_ref() == Some(root);
                let cap = if home_mode { home_budget } else { usize::MAX };
                set.add_tree(root, home_mode, cap, &*pred);
                written.push(files_written_since(root, *since, home_mode, cap, &*pred));
            }
            set.set_parents(&wanted);
            written
        })
        .await;
        let written = match done {
            Ok(written) => written,
            Err(e) => {
                self.ctx.status.set_failed(format!("watch setup panicked: {e}"));
                Vec::new()
            }
        };
        self.publish_watch_detail();
        for (root, _) in &back {
            if self.roots.workspaces.contains(root) {
                self.emit_workspace_seen(root).await?;
            }
        }
        for path in written.into_iter().flatten() {
            if let Some(root) = self.eligible(&path).await
                && self.coalescer.touch(path.clone(), Instant::now())
            {
                self.emit_file(path, &root).await?;
            }
        }
        Ok(())
    }

    fn root_of(&self, path: &Path) -> Option<(PathBuf, bool)> {
        let root = owning_root(path, &self.roots.all)?.clone();
        let home_mode = self.roots.home.as_ref() == Some(&root) && !self.roots.workspaces.contains(&root);
        Some((root, home_mode))
    }

    /// Eligible regular file → `(root)`; directories get watched as a side effect.
    async fn eligible(&mut self, path: &Path) -> Option<PathBuf> {
        let (root, home_mode) = self.root_of(path)?;
        if is_ignored_path(path, &root) || (home_mode && has_hidden_component(path, &root)) || (self.is_sensitive)(path)
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
                    let cap = if home_mode { self.opts.home_watch_budget } else { usize::MAX };
                    let _ =
                        tokio::task::spawn_blocking(move || lock(&set).add_tree(&dir, home_mode, cap, &*pred)).await;
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
        p.insert("path".into(), Value::String(path.to_string_lossy().into_owned()));
        p.insert("workspace".into(), Value::String(workspace.clone()));
        self.sink.emit("filesystem", "file_modify", p).await?;

        if !self.capture_content {
            return Ok(());
        }
        if self.inflight.load(Ordering::Relaxed) >= self.opts.content_queue_limit {
            self.content_dropped += 1;
            self.ctx.status.set_detail("content_dropped", self.content_dropped);
            return Ok(());
        }
        self.inflight.fetch_add(1, Ordering::Relaxed);
        let (sem, inflight, tools, sink, cancel) =
            (self.sem.clone(), self.inflight.clone(), self.tools.clone(), self.sink.clone(), self.ctx.cancel.clone());
        let max = self.opts.max_file_bytes;
        tokio::spawn(async move {
            let work = async {
                let _permit = sem.acquire().await.ok()?;
                let mut payload = content::extract(path, max, &tools).await?;
                payload.insert("workspace".into(), Value::String(workspace));
                sink.send(make_event("filesystem", "file_content", payload)).await.ok()
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
                let mut recheck = false;
                for action in classify_batch(&events) {
                    match action {
                        FsAction::Touched(path) => {
                            recheck |= self.affects_missing(&path);
                            if let Some(root) = self.eligible(&path).await
                                && self.coalescer.touch(path.clone(), Instant::now())
                            {
                                self.emit_file(path, &root).await?;
                            }
                        }
                        FsAction::Removed(path) => {
                            self.coalescer.forget(&path);
                            // Per-directory watches below `path` are gone. With one
                            // recursive watch per root, only a removed root matches.
                            if let Some(set) = &self.set {
                                lock(set).remove_tree(&path);
                            }
                            recheck |= self.mark_missing_under(&path) | self.affects_missing(&path);
                        }
                    }
                }
                if recheck {
                    self.recheck_missing().await?;
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

/// Most files reported for one root that comes back (a fresh clone has many).
const RECOVERED_FILES_LIMIT: usize = 256;

/// Regular files under `root` modified at or after `since` (less 2 s for coarse
/// mtimes), newest first, at most [`RECOVERED_FILES_LIMIT`].
fn files_written_since(
    root: &Path,
    since: SystemTime,
    skip_hidden: bool,
    cap: usize,
    is_sensitive: &dyn Fn(&Path) -> bool,
) -> Vec<PathBuf> {
    let since = since.checked_sub(Duration::from_secs(2)).unwrap_or(since);
    let (dirs, _) = plan_watch_dirs(root, skip_hidden, is_sensitive, cap);
    let mut files: Vec<(SystemTime, PathBuf)> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if let Ok(modified) = meta.modified()
                && meta.is_file()
                && modified >= since
            {
                files.push((modified, entry.path()));
            }
        }
    }
    files.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    files.into_iter().take(RECOVERED_FILES_LIMIT).map(|(_, path)| path).collect()
}

/// Treat everything below a `never_watch` folder as sensitive: it is never watched,
/// never a root and never reported.
fn with_never_watch(is_sensitive: SensitivePredicate, never_watch: &[PathBuf]) -> SensitivePredicate {
    if never_watch.is_empty() {
        return is_sensitive;
    }
    let dirs: Vec<PathBuf> =
        never_watch.iter().flat_map(|d| [d.clone(), d.canonicalize().unwrap_or_else(|_| d.clone())]).collect();
    Arc::new(move |path: &Path| dirs.iter().any(|d| path.starts_with(d)) || is_sensitive(path))
}

async fn sleep_until_opt(deadline: Option<Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
}

pub(crate) async fn run_with<W, F>(c: FilesystemCollector, factory: F, tools: Tools, ctx: CollectorContext)
where
    W: DirWatcher,
    F: Fn(mpsc::Sender<DebounceEventResult>, Duration) -> notify::Result<W>,
{
    let FilesystemCollector { config: mut config_rx, is_sensitive, options } = c;
    let is_sensitive = with_never_watch(is_sensitive, &options.never_watch);
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
        roots: Roots { workspaces: vec![], home: None, all: vec![], missing: vec![] },
        missing: BTreeMap::new(),
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
mod tests;
