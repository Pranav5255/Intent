//! Test harness: a real `intentd` router on an ephemeral port over a temp store.
#![allow(dead_code)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use intent_capture::Paths;
use intent_capture::restore::{LaunchCommand, SystemProbe};
use intentd::{AppState, Daemon, DaemonOptions};
use reqwest::{Method, StatusCode};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub fn paths_for(root: &Path) -> Paths {
    let root = root.to_path_buf();
    Paths::from_lookup(move |key| {
        let sub = match key {
            "HOME" => "home",
            "XDG_CONFIG_HOME" => "config",
            "XDG_DATA_HOME" => "data",
            "XDG_RUNTIME_DIR" => "run",
            _ => return None,
        };
        Some(OsString::from(root.join(sub)))
    })
}

/// Restore probe that pretends every program and path exists.
pub struct FakeProbe;

impl SystemProbe for FakeProbe {
    fn which(&self, program: &str) -> Option<PathBuf> {
        Some(PathBuf::from("/usr/bin").join(program))
    }
    fn is_file(&self, _: &Path) -> bool {
        true
    }
    fn is_dir(&self, _: &Path) -> bool {
        true
    }
    fn env(&self, _: &str) -> Option<String> {
        None
    }
    fn window_titles(&self) -> Option<String> {
        None
    }
}

pub struct TestDaemon {
    pub base: String,
    pub token: String,
    pub state: AppState,
    pub client: reqwest::Client,
    pub launched: Arc<Mutex<Vec<LaunchCommand>>>,
    stop: Option<oneshot::Sender<()>>,
    join: Option<JoinHandle<anyhow::Result<()>>>,
}

/// Options for tests: no collectors/jobs, a recording restore spawner, and the LLM
/// provider disabled unless the test writes its own `llm.toml`.
pub fn test_options(root: &Path) -> (DaemonOptions, Arc<Mutex<Vec<LaunchCommand>>>) {
    let paths = paths_for(root);
    let llm = paths.llm_config();
    if !llm.exists() {
        std::fs::create_dir_all(llm.parent().unwrap()).unwrap();
        std::fs::write(&llm, "provider = \"none\"\n").unwrap();
    }
    let mut opts = DaemonOptions::new(paths);
    opts.collectors = false;
    opts.jobs = false;
    let launched: Arc<Mutex<Vec<LaunchCommand>>> = Arc::default();
    let sink = Arc::clone(&launched);
    opts.restore_spawner = Arc::new(move |cmd: &LaunchCommand| {
        sink.lock().unwrap().push(cmd.clone());
        Ok(())
    });
    opts.restore_probe = Some(Arc::new(FakeProbe));
    (opts, launched)
}

impl TestDaemon {
    pub async fn start(root: &Path) -> TestDaemon {
        let (opts, launched) = test_options(root);
        Self::start_with(opts, launched).await
    }

    pub async fn start_with(mut opts: DaemonOptions, launched: Arc<Mutex<Vec<LaunchCommand>>>) -> TestDaemon {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        opts.bind = addr;
        let daemon = Daemon::start(opts).await.unwrap();
        let token = daemon.token().to_string();
        let state = Arc::clone(&daemon.state);
        let (stop, rx) = oneshot::channel::<()>();
        let join = tokio::spawn(daemon.serve(listener, async move {
            let _ = rx.await;
        }));
        TestDaemon {
            base: format!("http://{addr}"),
            token,
            state,
            client: reqwest::Client::new(),
            launched,
            stop: Some(stop),
            join: Some(join),
        }
    }

    pub async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(join) = self.join.take() {
            join.await.unwrap().unwrap();
        }
    }

    /// Authenticated request; returns status and parsed JSON body (Null when empty).
    pub async fn call(&self, method: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        self.request(method, path, body, true).await
    }

    pub async fn call_open(&self, method: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        self.request(method, path, body, false).await
    }

    pub async fn request(&self, method: Method, path: &str, body: Option<Value>, auth: bool) -> (StatusCode, Value) {
        let mut req = self.client.request(method, format!("{}{}", self.base, path));
        if auth {
            req = req.bearer_auth(&self.token);
        }
        if let Some(body) = body {
            req = req.json(&body);
        }
        let resp = req.send().await.unwrap();
        let status = resp.status();
        let bytes = resp.bytes().await.unwrap();
        let value = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap_or(Value::Null) };
        (status, value)
    }

    pub async fn post_event(&self, event: &Value) -> (StatusCode, Value) {
        self.call_open(Method::POST, "/v1/event", Some(event.clone())).await
    }
}

pub fn uuid() -> String {
    let mut b = [0u8; 16];
    for (i, x) in b.iter_mut().enumerate() {
        *x = (fastrand_u64() >> (i % 8 * 8)) as u8;
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

fn fastrand_u64() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEED: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64;
    let mut x = SEED.fetch_add(nanos | 1, Ordering::Relaxed) ^ nanos;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x
}

/// Unix timestamp of `hour:minute` local time, `days_ago` days before today.
pub fn local_ts(days_ago: u64, hour: u32, minute: u32) -> (String, i64) {
    use jiff::{ToSpan, tz::TimeZone};
    let date = jiff::Zoned::now().date().checked_sub((days_ago as i64).days()).unwrap();
    let at = date.at(hour as i8, minute as i8, 0, 0).to_zoned(TimeZone::system()).unwrap();
    (date.to_string(), at.timestamp().as_second())
}

/// Canonical path of a temp dir (macOS `/var` → `/private/var`), as the store resolves it.
pub fn canonical(path: &Path) -> String {
    std::fs::canonicalize(path).unwrap().to_string_lossy().into_owned()
}
