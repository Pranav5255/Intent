//! Foreground-window tracking → `linux/app_focus {app, title, window_id}`.
//!
//! Backends push raw [`FocusSample`]s into an internal channel; a shared driver
//! settles bursts (alt-tab cycling, animated titles), de-duplicates on the
//! `(window_id, app, title)` signature and emits events.

use std::sync::Arc;
use std::time::Duration;

use intent_types::Payload;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::feed::{ActivityFeed, ActivityKind};
use crate::session::{SessionEnv, SessionKind};
use crate::{Collector, CollectorContext, EventSink};

#[cfg(target_os = "linux")]
mod gnome;
#[cfg(target_os = "linux")]
mod x11;

#[cfg(target_os = "linux")]
pub use gnome::{DBUS_INTERFACE, DBUS_NAME, DBUS_PATH};

/// 1.x truncated titles to 512 characters.
pub const MAX_TITLE_CHARS: usize = 512;

/// One observation of the focused window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusSample {
    pub window_id: String,
    pub app: String,
    pub title: String,
}

impl FocusSample {
    pub fn new(window_id: impl Into<String>, app: impl Into<String>, title: &str) -> Self {
        Self { window_id: window_id.into(), app: app.into(), title: truncate_title(title) }
    }

    pub fn payload(&self) -> Payload {
        let mut p = Payload::new();
        p.insert("app".into(), Value::String(self.app.clone()));
        p.insert("title".into(), Value::String(self.title.clone()));
        p.insert("window_id".into(), Value::String(self.window_id.clone()));
        p
    }
}

pub fn truncate_title(title: &str) -> String {
    match title.char_indices().nth(MAX_TITLE_CHARS) {
        Some((idx, _)) => title[..idx].to_owned(),
        None => title.to_owned(),
    }
}

/// Map a WM_CLASS / Wayland app id to the stable names 1.x emitted.
pub fn friendly_app_name(raw: &str) -> String {
    let candidate = raw.trim().trim_end_matches(".desktop").to_lowercase();
    let mapped = match candidate.as_str() {
        "firefox" | "firefox_firefox" | "firefox-esr" | "org.mozilla.firefox" | "navigator" => "firefox",
        "google-chrome" | "google-chrome-stable" | "com.google.chrome" => "google-chrome",
        "chromium" | "chromium-browser" | "chromium_chromium" | "org.chromium.chromium" => "chromium",
        "code" | "code-url-handler" | "com.visualstudio.code" => "code",
        "gnome-terminal-server" | "gnome-terminal" | "org.gnome.terminal" => "gnome-terminal",
        "ptyxis" | "org.gnome.ptyxis" => "ptyxis",
        "org.gnome.nautilus" => "nautilus",
        "" => "unknown",
        other => other,
    };
    mapped.to_owned()
}

/// Parse a raw X11 `WM_CLASS` property (`instance\0class\0`) and map the class.
pub fn parse_wm_class(raw: &[u8]) -> String {
    let class = raw
        .split(|b| *b == 0)
        .rfind(|part| !part.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .unwrap_or_default();
    friendly_app_name(&class)
}

/// App name for a window reported by the GNOME extension: prefer `wm_class`
/// (matches what X11 sessions reported), fall back to the `.desktop` app id.
pub fn app_from_gnome(app_id: &str, wm_class: &str) -> String {
    if !wm_class.trim().is_empty() { friendly_app_name(wm_class) } else { friendly_app_name(app_id) }
}

// ---------------------------------------------------------------------------
// Settle + dedupe (pure, driven by explicit timestamps)
// ---------------------------------------------------------------------------

/// Coalesces bursts of focus/title changes and suppresses repeats.
///
/// A change is emitted once no further change arrived for `settle`, or after
/// `max_delay` of continuous churn (e.g. a progress bar in a terminal title),
/// and only if its signature differs from the last emitted one.
#[derive(Debug)]
pub struct FocusSettler {
    settle: Duration,
    max_delay: Duration,
    pending: Option<Pending>,
    last_emitted: Option<FocusSample>,
}

#[derive(Debug)]
struct Pending {
    sample: FocusSample,
    first_at: Instant,
    last_at: Instant,
}

impl FocusSettler {
    pub fn new(settle: Duration, max_delay: Duration) -> Self {
        Self { settle, max_delay, pending: None, last_emitted: None }
    }

    pub fn offer(&mut self, sample: FocusSample, now: Instant) {
        if self.last_emitted.as_ref() == Some(&sample) {
            // Back to what we already reported (e.g. alt-tab and back): drop the transient.
            self.pending = None;
            return;
        }
        match &mut self.pending {
            Some(p) if p.sample == sample => {}
            Some(p) => {
                p.sample = sample;
                p.last_at = now;
            }
            None => self.pending = Some(Pending { sample, first_at: now, last_at: now }),
        }
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.pending.as_ref().map(|p| (p.last_at + self.settle).min(p.first_at + self.max_delay))
    }

    pub fn poll(&mut self, now: Instant) -> Option<FocusSample> {
        if self.deadline()? > now {
            return None;
        }
        let p = self.pending.take()?;
        self.last_emitted = Some(p.sample.clone());
        Some(p.sample)
    }
}

/// Drain samples from a backend until it closes the channel or we are cancelled.
/// Returns `Err(SinkClosed)` if the daemon side went away.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) async fn drive(
    mut rx: mpsc::Receiver<FocusSample>,
    sink: &EventSink,
    feed: Option<&ActivityFeed>,
    settler: &mut FocusSettler,
) -> Result<(), crate::SinkClosed> {
    loop {
        let deadline = settler.deadline();
        tokio::select! {
            sample = rx.recv() => match sample {
                Some(s) => settler.offer(s, Instant::now()),
                None => return Ok(()),
            },
            _ = sleep_until_opt(deadline) => {
                if let Some(s) = settler.poll(Instant::now()) {
                    if let Some(feed) = feed {
                        feed.record(ActivityKind::Focus);
                    }
                    sink.emit("linux", "app_focus", s.payload()).await?;
                }
            }
        }
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
async fn sleep_until_opt(deadline: Option<Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
}

// ---------------------------------------------------------------------------
// Backend selection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusBackendPref {
    #[default]
    Auto,
    X11,
    Gnome,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Candidate {
    X11,
    /// `wait`: if the extension is not on the bus yet, stay in `Waiting`
    /// (zero-cost NameOwnerChanged subscription) instead of giving up.
    Gnome {
        wait: bool,
    },
}

/// Ordered list of backends to try. Never uses X11 inside a Wayland session
/// (XWayland only sees X11 clients).
pub(crate) fn candidates(pref: FocusBackendPref, env: &SessionEnv) -> Vec<Candidate> {
    let kind = env.kind();
    let gnome = env.is_gnome();
    match pref {
        FocusBackendPref::Disabled => vec![],
        FocusBackendPref::X11 => vec![Candidate::X11],
        FocusBackendPref::Gnome => vec![Candidate::Gnome { wait: true }],
        FocusBackendPref::Auto => match kind {
            SessionKind::Wayland => vec![Candidate::Gnome { wait: gnome }],
            SessionKind::X11 if gnome => vec![Candidate::X11, Candidate::Gnome { wait: true }],
            SessionKind::X11 => vec![Candidate::X11, Candidate::Gnome { wait: false }],
            SessionKind::Unknown if gnome => vec![Candidate::Gnome { wait: false }],
            SessionKind::Unknown => vec![],
        },
    }
}

#[derive(Debug, Clone)]
pub struct FocusConfig {
    pub backend: FocusBackendPref,
    /// Quiet period before a change is reported.
    pub settle: Duration,
    /// Upper bound on how long continuous churn can delay a report.
    pub max_delay: Duration,
}

impl Default for FocusConfig {
    fn default() -> Self {
        Self { backend: FocusBackendPref::Auto, settle: Duration::from_millis(400), max_delay: Duration::from_secs(5) }
    }
}

/// Emits `linux/app_focus`. Optionally marks focus activity on a shared feed.
#[derive(Debug, Clone, Default)]
pub struct FocusCollector {
    pub config: FocusConfig,
    pub env: Option<SessionEnv>,
    pub feed: Option<Arc<ActivityFeed>>,
}

impl FocusCollector {
    pub fn new(config: FocusConfig, feed: Option<Arc<ActivityFeed>>) -> Self {
        Self { config, env: None, feed }
    }
}

/// Outcome of trying one backend.
#[allow(dead_code)]
pub(crate) enum BackendEnd {
    /// Could not attach; try the next candidate.
    NotAvailable(String),
    /// Ran and then lost its source (X server gone, ...). Terminal.
    Lost(String),
    /// Cancelled or sink closed.
    Stopped,
}

impl Collector for FocusCollector {
    fn name(&self) -> &'static str {
        "focus"
    }

    async fn run(self, ctx: CollectorContext) {
        let env = self.env.clone().unwrap_or_else(SessionEnv::from_env);
        let list = candidates(self.config.backend, &env);
        if list.is_empty() {
            let why = if self.config.backend == FocusBackendPref::Disabled {
                "disabled by configuration"
            } else {
                "no graphical session detected (DISPLAY / WAYLAND_DISPLAY unset)"
            };
            ctx.status.set_unavailable(why);
            return;
        }
        let mut reasons = Vec::new();
        for cand in list {
            match run_candidate(cand, &self, &ctx, &env).await {
                BackendEnd::NotAvailable(why) => reasons.push(why),
                BackendEnd::Lost(why) => {
                    ctx.status.set_unavailable(why);
                    return;
                }
                BackendEnd::Stopped => return,
            }
        }
        ctx.status.set_unavailable(reasons.join("; "));
    }
}

#[cfg(target_os = "linux")]
async fn run_candidate(cand: Candidate, c: &FocusCollector, ctx: &CollectorContext, env: &SessionEnv) -> BackendEnd {
    let (tx, rx) = mpsc::channel::<FocusSample>(64);
    let mut settler = FocusSettler::new(c.config.settle, c.config.max_delay);
    let backend = async {
        match cand {
            Candidate::X11 => x11::run(env, tx, &ctx.status, &ctx.cancel).await,
            Candidate::Gnome { wait } => gnome::run(wait, tx, &ctx.status, &ctx.cancel).await,
        }
    };
    let driver = drive(rx, &ctx.sink, c.feed.as_deref(), &mut settler);
    tokio::pin!(backend, driver);
    tokio::select! {
        biased;
        end = &mut backend => {
            // Flush anything already queued by the backend before reporting.
            match end {
                BackendEnd::NotAvailable(_) => end,
                other => { let _ = tokio::time::timeout(Duration::from_millis(10), &mut driver).await; other }
            }
        }
        // Ok: backend dropped its sender (handled above); Err: sink closed.
        _ = &mut driver => BackendEnd::Stopped,
        _ = ctx.cancel.cancelled() => BackendEnd::Stopped,
    }
}

#[cfg(not(target_os = "linux"))]
async fn run_candidate(
    _cand: Candidate,
    _c: &FocusCollector,
    _ctx: &CollectorContext,
    _env: &SessionEnv,
) -> BackendEnd {
    BackendEnd::NotAvailable("focus tracking is only implemented on Linux".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::{CollectorState, StatusHandle};

    #[test]
    fn wm_class_parsing_matches_1x() {
        assert_eq!(parse_wm_class(b"Navigator\0firefox\0"), "firefox");
        assert_eq!(parse_wm_class(b"gnome-terminal-server\0Gnome-terminal\0"), "gnome-terminal");
        assert_eq!(parse_wm_class(b"google-chrome\0Google-chrome\0"), "google-chrome");
        assert_eq!(parse_wm_class(b"code\0Code\0"), "code");
        assert_eq!(parse_wm_class(b"slack\0Slack\0"), "slack");
        assert_eq!(parse_wm_class(b""), "unknown");
        assert_eq!(parse_wm_class(b"onlyinstance"), "onlyinstance");
    }

    #[test]
    fn wayland_app_ids_map_to_same_names() {
        assert_eq!(app_from_gnome("firefox_firefox.desktop", ""), "firefox");
        assert_eq!(app_from_gnome("org.gnome.Terminal.desktop", "gnome-terminal-server"), "gnome-terminal");
        assert_eq!(app_from_gnome("code.desktop", "Code"), "code");
        assert_eq!(app_from_gnome("org.gnome.Ptyxis.desktop", "org.gnome.Ptyxis"), "ptyxis");
        assert_eq!(app_from_gnome("", ""), "unknown");
    }

    #[test]
    fn titles_truncate_on_char_boundary() {
        let long = "é".repeat(600);
        let t = truncate_title(&long);
        assert_eq!(t.chars().count(), MAX_TITLE_CHARS);
        assert_eq!(truncate_title("short"), "short");
    }

    #[test]
    fn payload_shape() {
        let p = FocusSample::new("123", "code", "main.rs").payload();
        assert_eq!(
            serde_json::to_value(&p).unwrap(),
            serde_json::json!({"app":"code","title":"main.rs","window_id":"123"})
        );
    }

    fn s(id: &str, title: &str) -> FocusSample {
        FocusSample::new(id, "app", title)
    }

    #[test]
    fn settler_dedupes_and_coalesces() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut st = FocusSettler::new(Duration::from_millis(400), Duration::from_secs(5));
        assert_eq!(st.deadline(), None);
        st.offer(s("1", "a"), ms(0));
        assert_eq!(st.poll(ms(399)), None);
        assert_eq!(st.poll(ms(400)), Some(s("1", "a")));
        // identical signature: nothing pending
        st.offer(s("1", "a"), ms(500));
        assert_eq!(st.deadline(), None);
        // alt-tab burst: only the final window is reported
        st.offer(s("2", "b"), ms(1000));
        st.offer(s("3", "c"), ms(1100));
        st.offer(s("4", "d"), ms(1200));
        assert_eq!(st.poll(ms(1500)), None);
        assert_eq!(st.poll(ms(1600)), Some(s("4", "d")));
        // transient switch away and back is dropped entirely
        st.offer(s("5", "e"), ms(2000));
        st.offer(s("4", "d"), ms(2100));
        assert_eq!(st.deadline(), None);
        // title change on same window is a new signature
        st.offer(s("4", "d2"), ms(3000));
        assert_eq!(st.poll(ms(3400)), Some(s("4", "d2")));
    }

    #[test]
    fn settler_bounds_continuous_churn() {
        let t0 = Instant::now();
        let mut st = FocusSettler::new(Duration::from_millis(400), Duration::from_secs(2));
        for i in 0..30u64 {
            st.offer(s("1", &format!("progress {i}%")), t0 + Duration::from_millis(i * 100));
            if i * 100 >= 2000 {
                break;
            }
        }
        assert!(st.deadline().unwrap() <= t0 + Duration::from_secs(2));
        assert!(st.poll(t0 + Duration::from_secs(2)).is_some());
    }

    #[test]
    fn candidate_selection() {
        let wl_gnome = SessionEnv {
            session_type: Some("wayland".into()),
            wayland_display: Some("wayland-0".into()),
            display: Some(":0".into()),
            current_desktop: Some("ubuntu:GNOME".into()),
        };
        assert_eq!(candidates(FocusBackendPref::Auto, &wl_gnome), vec![Candidate::Gnome { wait: true }]);
        let wl_kde = SessionEnv { current_desktop: Some("KDE".into()), ..wl_gnome.clone() };
        assert_eq!(candidates(FocusBackendPref::Auto, &wl_kde), vec![Candidate::Gnome { wait: false }]);
        let x_gnome = SessionEnv {
            session_type: Some("x11".into()),
            wayland_display: None,
            display: Some(":0".into()),
            current_desktop: Some("ubuntu:GNOME".into()),
        };
        assert_eq!(candidates(FocusBackendPref::Auto, &x_gnome), vec![Candidate::X11, Candidate::Gnome { wait: true }]);
        assert!(candidates(FocusBackendPref::Auto, &SessionEnv::default()).is_empty());
        assert!(candidates(FocusBackendPref::Disabled, &x_gnome).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn driver_emits_settled_events_from_fake_backend() {
        let (etx, mut erx) = mpsc::channel(16);
        let sink = EventSink::new(etx, StatusHandle::new("focus"));
        let feed = ActivityFeed::new();
        feed.set_idle_ms(99_000);
        let (tx, rx) = mpsc::channel(16);
        let mut settler = FocusSettler::new(Duration::from_millis(400), Duration::from_secs(5));
        let backend = async move {
            tx.send(s("1", "a")).await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            tx.send(s("1", "b")).await.unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
            tx.send(s("1", "b")).await.unwrap();
            tx.send(s("2", "c")).await.unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        };
        let (_, res) = tokio::join!(backend, drive(rx, &sink, Some(&feed), &mut settler));
        res.unwrap();
        let a = erx.recv().await.unwrap();
        assert_eq!((a.source.as_str(), a.kind.as_str()), ("linux", "app_focus"));
        assert_eq!(a.payload["title"], "b");
        let b = erx.recv().await.unwrap();
        assert_eq!(b.payload["window_id"], "2");
        assert!(erx.try_recv().is_err());
        assert_eq!(feed.idle_ms(), 0, "focus change counts as activity");
    }

    #[tokio::test]
    async fn disabled_reports_unavailable_and_exits() {
        let (tx, _rx) = mpsc::channel(1);
        let c = FocusCollector {
            config: FocusConfig { backend: FocusBackendPref::Disabled, ..Default::default() },
            ..Default::default()
        };
        let h = crate::spawn(c, tx, crate::CancellationToken::new());
        h.join.await.unwrap();
        assert_eq!(h.status.snapshot().state, CollectorState::Unavailable);
    }

    #[cfg(not(target_os = "linux"))]
    #[tokio::test]
    async fn non_linux_is_unavailable() {
        let (tx, _rx) = mpsc::channel(1);
        let env = SessionEnv { display: Some(":0".into()), ..Default::default() };
        let c = FocusCollector { env: Some(env), ..Default::default() };
        let h = crate::spawn(c, tx, crate::CancellationToken::new());
        h.join.await.unwrap();
        let st = h.status.snapshot();
        assert_eq!(st.state, CollectorState::Unavailable);
        assert!(st.error.unwrap().contains("Linux"));
    }
}
