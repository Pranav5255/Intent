//! User idle detection → `linux/idle_start` / `linux/idle_end` (empty payloads, as in 1.x).
//!
//! Backends:
//! * `mutter-idle-monitor` — `org.gnome.Mutter.IdleMonitor` on the session bus
//!   (GNOME on Wayland *and* X11). Fully event-driven via idle/user-active watches.
//! * `x11-screensaver` — MIT-SCREEN-SAVER `QueryInfo` over `x11rb` (non-GNOME X11).
//!   A single X round-trip per check, scheduled to land on the threshold crossing.
//!
//! Every backend also feeds the current idle time into the shared
//! [`ActivityFeed`] so its `recommended_poll_interval` keeps 1.x semantics.

use std::sync::Arc;
use std::time::Duration;

use intent_types::Payload;

use crate::feed::ActivityFeed;
use crate::session::{SessionEnv, SessionKind};
use crate::{Collector, CollectorContext, EventSink, SinkClosed};

#[cfg(target_os = "linux")]
mod mutter;
#[cfg(target_os = "linux")]
mod x11;

pub const DEFAULT_IDLE_THRESHOLD: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleTransition {
    Start,
    End,
}

impl IdleTransition {
    pub fn event_type(self) -> &'static str {
        match self {
            IdleTransition::Start => "idle_start",
            IdleTransition::End => "idle_end",
        }
    }
}

/// Threshold state machine shared by all backends.
#[derive(Debug, Clone)]
pub struct IdleTracker {
    threshold_ms: u64,
    idle: bool,
}

impl IdleTracker {
    pub fn new(threshold_ms: u64) -> Self {
        Self { threshold_ms: threshold_ms.max(1), idle: false }
    }

    pub fn is_idle(&self) -> bool {
        self.idle
    }

    pub fn threshold_ms(&self) -> u64 {
        self.threshold_ms
    }

    pub fn observe(&mut self, idle_ms: u64) -> Option<IdleTransition> {
        let now_idle = idle_ms >= self.threshold_ms;
        if now_idle == self.idle {
            return None;
        }
        self.idle = now_idle;
        Some(if now_idle { IdleTransition::Start } else { IdleTransition::End })
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) async fn emit(sink: &EventSink, t: IdleTransition) -> Result<(), SinkClosed> {
    sink.emit("linux", t.event_type(), Payload::new()).await
}

/// Delay until the next X11 idle query: right after the threshold would be
/// crossed while active (capped at 10s so the feed stays fresh), 2s while idle
/// so `idle_end` is prompt.
pub fn x11_poll_delay(idle_ms: u64, threshold_ms: u64, is_idle: bool) -> Duration {
    if is_idle {
        return Duration::from_secs(2);
    }
    let until = threshold_ms.saturating_sub(idle_ms) + 50;
    Duration::from_millis(until.clamp(100, 10_000))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IdleBackendPref {
    #[default]
    Auto,
    Mutter,
    X11,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Candidate {
    Mutter,
    X11,
}

pub(crate) fn candidates(pref: IdleBackendPref, env: &SessionEnv) -> Vec<Candidate> {
    match pref {
        IdleBackendPref::Disabled => vec![],
        IdleBackendPref::Mutter => vec![Candidate::Mutter],
        IdleBackendPref::X11 => vec![Candidate::X11],
        IdleBackendPref::Auto => match env.kind() {
            // XWayland's screensaver extension only sees X11 clients' input.
            SessionKind::Wayland => vec![Candidate::Mutter],
            SessionKind::X11 => vec![Candidate::Mutter, Candidate::X11],
            SessionKind::Unknown if env.is_gnome() => vec![Candidate::Mutter],
            SessionKind::Unknown => vec![],
        },
    }
}

#[derive(Debug, Clone)]
pub struct IdleConfig {
    pub threshold: Duration,
    pub backend: IdleBackendPref,
}

impl Default for IdleConfig {
    fn default() -> Self {
        let threshold = std::env::var("INTENT_IDLE_THRESHOLD_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or(DEFAULT_IDLE_THRESHOLD);
        Self { threshold, backend: IdleBackendPref::Auto }
    }
}

#[derive(Debug, Clone)]
pub struct IdleCollector {
    pub config: IdleConfig,
    pub env: Option<SessionEnv>,
    pub feed: Arc<ActivityFeed>,
}

impl IdleCollector {
    pub fn new(config: IdleConfig, feed: Arc<ActivityFeed>) -> Self {
        Self { config, env: None, feed }
    }
}

#[allow(dead_code)]
pub(crate) enum BackendEnd {
    NotAvailable(String),
    Lost(String),
    Stopped,
}

impl Collector for IdleCollector {
    fn name(&self) -> &'static str {
        "idle"
    }

    async fn run(self, ctx: CollectorContext) {
        let env = self.env.clone().unwrap_or_else(SessionEnv::from_env);
        let threshold_ms = self.config.threshold.as_millis() as u64;
        ctx.status.set_detail("threshold_ms", threshold_ms);
        let list = candidates(self.config.backend, &env);
        if list.is_empty() {
            ctx.status.set_unavailable(if self.config.backend == IdleBackendPref::Disabled {
                "disabled by configuration"
            } else {
                "no supported idle source (needs GNOME/Mutter or an X11 session)"
            });
            return;
        }
        let mut reasons = Vec::new();
        for cand in list {
            match run_candidate(cand, &self, &ctx, &env, threshold_ms).await {
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
async fn run_candidate(
    cand: Candidate,
    c: &IdleCollector,
    ctx: &CollectorContext,
    env: &SessionEnv,
    threshold_ms: u64,
) -> BackendEnd {
    match cand {
        Candidate::Mutter => mutter::run(threshold_ms, &c.feed, ctx).await,
        Candidate::X11 => x11::run(env, threshold_ms, &c.feed, ctx).await,
    }
}

#[cfg(not(target_os = "linux"))]
async fn run_candidate(
    _cand: Candidate,
    _c: &IdleCollector,
    _ctx: &CollectorContext,
    _env: &SessionEnv,
    _threshold_ms: u64,
) -> BackendEnd {
    BackendEnd::NotAvailable("idle detection is only implemented on Linux".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::{CollectorState, StatusHandle};
    use tokio::sync::mpsc;

    #[test]
    fn state_machine_emits_on_edges_only() {
        let mut t = IdleTracker::new(300_000);
        assert_eq!(t.observe(0), None);
        assert_eq!(t.observe(299_999), None);
        assert_eq!(t.observe(300_000), Some(IdleTransition::Start));
        assert!(t.is_idle());
        assert_eq!(t.observe(900_000), None);
        assert_eq!(t.observe(10), Some(IdleTransition::End));
        assert_eq!(t.observe(10), None);
        assert_eq!(IdleTransition::Start.event_type(), "idle_start");
        assert_eq!(IdleTransition::End.event_type(), "idle_end");
    }

    #[test]
    fn x11_delay_targets_threshold() {
        assert_eq!(x11_poll_delay(0, 300_000, false), Duration::from_secs(10));
        assert_eq!(x11_poll_delay(299_000, 300_000, false), Duration::from_millis(1_050));
        assert_eq!(x11_poll_delay(300_000, 300_000, false), Duration::from_millis(100));
        assert_eq!(x11_poll_delay(400_000, 300_000, true), Duration::from_secs(2));
    }

    #[test]
    fn candidates_never_use_x11_on_wayland() {
        let wl = SessionEnv {
            session_type: Some("wayland".into()),
            display: Some(":0".into()),
            wayland_display: Some("wayland-0".into()),
            current_desktop: Some("GNOME".into()),
        };
        assert_eq!(candidates(IdleBackendPref::Auto, &wl), vec![Candidate::Mutter]);
        let x = SessionEnv { session_type: Some("x11".into()), display: Some(":0".into()), ..Default::default() };
        assert_eq!(candidates(IdleBackendPref::Auto, &x), vec![Candidate::Mutter, Candidate::X11]);
        assert!(candidates(IdleBackendPref::Auto, &SessionEnv::default()).is_empty());
    }

    #[tokio::test]
    async fn emits_1x_shapes() {
        let (tx, mut rx) = mpsc::channel(2);
        let sink = EventSink::new(tx, StatusHandle::new("idle"));
        emit(&sink, IdleTransition::Start).await.unwrap();
        let ev = rx.recv().await.unwrap();
        assert_eq!((ev.source.as_str(), ev.kind.as_str()), ("linux", "idle_start"));
        assert!(ev.payload.is_empty());
    }

    #[tokio::test]
    async fn no_session_is_unavailable_not_crash() {
        let (tx, _rx) = mpsc::channel(1);
        let c =
            IdleCollector { config: IdleConfig::default(), env: Some(SessionEnv::default()), feed: Default::default() };
        let h = crate::spawn(c, tx, crate::CancellationToken::new());
        h.join.await.unwrap();
        let st = h.status.snapshot();
        assert_eq!(st.state, CollectorState::Unavailable);
        assert!(st.detail.contains_key("threshold_ms"));
    }
}
