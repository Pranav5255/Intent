//! Filesystem collector tests (fake watcher; one ignored real-backend smoke test).

use super::*;
use crate::status::{CollectorState, StatusHandle};
use intent_types::RawEvent;
use notify::event::{CreateKind, DataChange, ModifyKind, RemoveKind, RenameMode};
use notify::{Event, EventKind};
use notify_debouncer_full::DebouncedEvent;

#[test]
fn plan_skips_ignored_hidden_and_respects_cap() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    for d in ["src/a", "node_modules/x", ".git/objects", ".config/app", "secret/k", "docs"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("docs"), root.join("docs-link")).unwrap();
    let pred = |p: &Path| p.ends_with("secret");
    let (dirs, truncated) = plan_watch_dirs(&root, false, &pred, usize::MAX);
    assert!(!truncated);
    let rel: HashSet<String> =
        dirs.iter().map(|d| d.strip_prefix(&root).unwrap().to_string_lossy().into_owned()).collect();
    let expect: HashSet<String> =
        ["", "src", "src/a", "docs", ".config", ".config/app"].iter().map(|s| s.to_string()).collect();
    assert_eq!(rel, expect);
    let (dirs, _) = plan_watch_dirs(&root, true, &pred, usize::MAX);
    assert!(dirs.iter().all(|d| !d.to_string_lossy().contains(".config")), "hidden skipped in home mode");
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
        .map(|(k, p)| DebouncedEvent::new(Event::new(*k).add_path(p.to_path_buf()), std::time::Instant::now()))
        .collect())
}

async fn next(rx: &mut mpsc::Receiver<RawEvent>) -> RawEvent {
    tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("event in time").unwrap()
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
    let pred: SensitivePredicate = Arc::new(|p: &Path| p.file_name().is_some_and(|n| n == ".env"));
    let opts = FsOptions { per_path_interval: Duration::from_secs(60), home: None, ..Default::default() };
    let (tx, mut rx) = mpsc::channel(32);
    let cancel = crate::CancellationToken::new();
    let h = crate::spawn(TestCollector(FilesystemCollector::new(cfg_rx, pred, opts), fake.clone()), tx, cancel.clone());

    let seen = next(&mut rx).await;
    assert_eq!((seen.source.as_str(), seen.kind.as_str()), ("filesystem", "workspace_seen"));
    assert_eq!(seen.payload["workspace"], ws1.to_string_lossy().as_ref());
    let st = h.status.snapshot();
    assert_eq!(st.state, CollectorState::Running);
    assert_eq!(st.detail["rejected_roots"][0]["path"], "/");
    if PER_DIR {
        assert!(!lock(&fake.watched).iter().any(|p| p.ends_with("node_modules")));
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
    assert_eq!(m.payload["path"], ws1.join("a.md").to_string_lossy().as_ref());
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
    assert_eq!(m2.payload["path"], ws2.join("b.txt").to_string_lossy().as_ref());
    // No content event (capture turned off) and the coalesced a.md write produced nothing.
    assert!(tokio::time::timeout(Duration::from_millis(200), rx.recv()).await.is_err());

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
    let (_cfg_tx, cfg_rx) =
        watch::channel(WatcherConfig { workspaces: vec![ws.clone()], all_accessible: false, capture_content: false });
    let opts = FsOptions { debounce: Duration::from_millis(200), ..Default::default() };
    let (tx, mut rx) = mpsc::channel(32);
    let cancel = crate::CancellationToken::new();
    let h = crate::spawn(FilesystemCollector::new(cfg_rx, Arc::new(|_: &Path| false), opts), tx, cancel.clone());
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

async fn wait_for(what: &str, mut check: impl FnMut() -> bool) {
    for _ in 0..500 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

fn missing_roots(h: &crate::CollectorHandle) -> Vec<String> {
    h.status
        .snapshot()
        .detail
        .get("missing_roots")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

struct Harness {
    fake: Fake,
    rx: mpsc::Receiver<RawEvent>,
    h: crate::CollectorHandle,
    cancel: crate::CancellationToken,
    _cfg: watch::Sender<WatcherConfig>,
}

impl Harness {
    fn start(workspaces: Vec<PathBuf>, options: FsOptions) -> Harness {
        let (cfg, cfg_rx) = watch::channel(WatcherConfig { workspaces, all_accessible: false, capture_content: false });
        let fake = Fake::default();
        let (tx, rx) = mpsc::channel(32);
        let cancel = crate::CancellationToken::new();
        let c = FilesystemCollector::new(cfg_rx, Arc::new(|_: &Path| false), options);
        let h = crate::spawn(TestCollector(c, fake.clone()), tx, cancel.clone());
        Harness { fake, rx, h, cancel, _cfg: cfg }
    }

    fn watched(&self, path: &Path) -> bool {
        lock(&self.fake.watched).iter().any(|p| p == path)
    }

    async fn stop(self) {
        self.cancel.cancel();
        self.h.join.await.unwrap();
    }
}

fn test_options() -> FsOptions {
    FsOptions { per_path_interval: Duration::from_secs(60), home: None, ..Default::default() }
}

#[tokio::test]
async fn deleted_root_is_watched_again_when_it_comes_back() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let ws = base.join("src");
    std::fs::create_dir_all(ws.join("proj")).unwrap();
    let mut t = Harness::start(vec![ws.clone()], test_options());
    assert_eq!(next(&mut t.rx).await.kind, "workspace_seen");
    assert!(t.watched(&ws));

    // `rm -rf src`: the root's own watch reports the removal.
    std::fs::remove_dir_all(&ws).unwrap();
    inject(&t.fake, batch(&[(EventKind::Remove(RemoveKind::Folder), &ws)])).await;
    let want = vec![ws.to_string_lossy().into_owned()];
    wait_for("the missing root in status", || missing_roots(&t.h) == want).await;
    assert!(t.watched(&base), "the nearest existing parent is watched");
    assert!(!t.watched(&ws));

    // `mkdir -p src/proj && echo x > src/proj/main.rs`.
    std::fs::create_dir_all(ws.join("proj")).unwrap();
    std::fs::write(ws.join("proj/main.rs"), "fn main() {}").unwrap();
    inject(&t.fake, batch(&[(EventKind::Create(CreateKind::Folder), &ws)])).await;
    let seen = next(&mut t.rx).await;
    assert_eq!(seen.kind, "workspace_seen");
    assert_eq!(seen.payload["workspace"], ws.to_string_lossy().as_ref());
    assert!(t.watched(&ws));
    if PER_DIR {
        assert!(t.watched(&ws.join("proj")), "the whole tree is watched again");
    }
    assert!(!t.watched(&base), "the parent watch is dropped");
    assert!(missing_roots(&t.h).is_empty());
    // main.rs was written before its folder was watched again; it is still reported.
    let m = next(&mut t.rx).await;
    assert_eq!(m.kind, "file_modify");
    assert_eq!(m.payload["path"], ws.join("proj/main.rs").to_string_lossy().as_ref());
    std::fs::write(ws.join("proj/lib.rs"), "").unwrap();
    inject(&t.fake, batch(&[(EventKind::Create(CreateKind::File), &ws.join("proj/lib.rs"))])).await;
    assert_eq!(next(&mut t.rx).await.payload["path"], ws.join("proj/lib.rs").to_string_lossy().as_ref());
    t.stop().await;
}

#[tokio::test]
async fn a_root_that_is_back_before_its_removal_is_seen_is_rewatched() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let ws = base.join("src");
    std::fs::create_dir_all(&ws).unwrap();
    let mut t = Harness::start(vec![ws.clone()], test_options());
    assert_eq!(next(&mut t.rx).await.kind, "workspace_seen");
    // `rm -rf src && mkdir -p src/proj && echo x > src/proj/main.rs`, all inside one
    // debounce window: only the removal of the old root is reported.
    std::fs::remove_dir_all(&ws).unwrap();
    std::fs::create_dir_all(ws.join("proj")).unwrap();
    std::fs::write(ws.join("proj/main.rs"), "fn main() {}").unwrap();
    inject(&t.fake, batch(&[(EventKind::Remove(RemoveKind::Folder), &ws)])).await;
    assert_eq!(next(&mut t.rx).await.kind, "workspace_seen");
    let m = next(&mut t.rx).await;
    assert_eq!(m.payload["path"], ws.join("proj/main.rs").to_string_lossy().as_ref());
    assert!(t.watched(&ws));
    assert!(!t.watched(&base));
    assert!(missing_roots(&t.h).is_empty());
    t.stop().await;
}

#[tokio::test]
async fn renamed_root_is_watched_again_when_renamed_back() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let ws = base.join("proj");
    std::fs::create_dir_all(&ws).unwrap();
    let mut t = Harness::start(vec![ws.clone()], test_options());
    assert_eq!(next(&mut t.rx).await.kind, "workspace_seen");

    // `mv proj proj2`: inotify reports MOVE_SELF as a rename "from".
    std::fs::rename(&ws, base.join("proj2")).unwrap();
    inject(&t.fake, batch(&[(EventKind::Modify(ModifyKind::Name(RenameMode::From)), &ws)])).await;
    let want = vec![ws.to_string_lossy().into_owned()];
    wait_for("the missing root in status", || missing_roots(&t.h) == want).await;
    assert!(t.watched(&base));

    std::fs::rename(base.join("proj2"), &ws).unwrap();
    inject(&t.fake, batch(&[(EventKind::Modify(ModifyKind::Name(RenameMode::To)), &ws)])).await;
    assert_eq!(next(&mut t.rx).await.kind, "workspace_seen");
    assert!(t.watched(&ws));
    assert!(missing_roots(&t.h).is_empty());
    t.stop().await;
}

#[tokio::test]
async fn never_watches_intents_own_folders() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let own = base.join("data/intent");
    std::fs::create_dir_all(own.join("logs")).unwrap();
    std::fs::create_dir_all(base.join("src")).unwrap();
    std::fs::write(own.join("intent.db-wal"), "x").unwrap();
    std::fs::write(base.join("src/a.rs"), "x").unwrap();
    let options = FsOptions { never_watch: vec![own.clone()], ..test_options() };
    let mut t = Harness::start(vec![base.clone(), own.clone()], options);
    assert_eq!(next(&mut t.rx).await.kind, "workspace_seen");
    let detail = t.h.status.snapshot().detail;
    assert_eq!(detail["rejected_roots"][0]["path"], own.to_string_lossy().as_ref());
    if PER_DIR {
        assert!(t.watched(&base.join("src")));
        assert!(!lock(&t.fake.watched).iter().any(|p| p.starts_with(&own)), "own folder watched");
    }
    let data = EventKind::Modify(ModifyKind::Data(DataChange::Content));
    inject(&t.fake, batch(&[(data, &own.join("intent.db-wal")), (data, &base.join("src/a.rs"))])).await;
    let m = next(&mut t.rx).await;
    assert_eq!(m.payload["path"], base.join("src/a.rs").to_string_lossy().as_ref());
    assert!(tokio::time::timeout(Duration::from_millis(200), t.rx.recv()).await.is_err());
    t.stop().await;
}

#[tokio::test]
async fn missing_root_at_start_is_picked_up_when_created() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    let ws = base.join("later/proj");
    let mut t = Harness::start(vec![ws.clone()], test_options());
    let want = vec![ws.to_string_lossy().into_owned()];
    wait_for("the missing root in status", || missing_roots(&t.h) == want).await;
    assert!(t.h.status.snapshot().running);
    assert!(t.watched(&base));

    // `mkdir -p later/proj` seen in two steps: the watch moves down to `later` first.
    std::fs::create_dir_all(base.join("later")).unwrap();
    inject(&t.fake, batch(&[(EventKind::Create(CreateKind::Folder), &base.join("later"))])).await;
    wait_for("the watch to move to the new parent", || t.watched(&base.join("later"))).await;
    assert!(!t.watched(&base));
    std::fs::create_dir_all(&ws).unwrap();
    inject(&t.fake, batch(&[(EventKind::Create(CreateKind::Folder), &ws)])).await;
    let seen = next(&mut t.rx).await;
    assert_eq!(seen.kind, "workspace_seen");
    assert_eq!(seen.payload["workspace"], ws.to_string_lossy().as_ref());
    assert!(t.watched(&ws));
    assert!(!t.watched(&base.join("later")));
    t.stop().await;
}
