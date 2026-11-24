//! Privacy-preserving activity timing signals (port of 0.1 `collectors/activity/feed.py`).
//!
//! No key values, mouse coordinates, or window content are retained: only event
//! times and coarse kinds, used to tune polling and expose capture health.

use std::collections::VecDeque;
use std::str::FromStr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKind {
    Key,
    Mouse,
    Shell,
    Focus,
}

impl FromStr for ActivityKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "key" => Ok(Self::Key),
            "mouse" => Ok(Self::Mouse),
            "shell" => Ok(Self::Shell),
            "focus" => Ok(Self::Focus),
            _ => Err("activity kind must be key, mouse, shell, or focus".into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct FeedSnapshot {
    pub idle_ms: u64,
    pub is_typing: bool,
    pub is_keyboard_burst: bool,
    pub recommended_poll_interval_sec: f64,
}

type Clock = Box<dyn Fn() -> Instant + Send + Sync>;

struct Inner {
    last_activity_at: Instant,
    last_key_at: Option<Instant>,
    key_times: VecDeque<Instant>,
    reported_idle_ms: Option<u64>,
}

/// Thread-safe activity feed. Share it as `Arc<ActivityFeed>`.
pub struct ActivityFeed {
    clock: Clock,
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for ActivityFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActivityFeed").field("snapshot", &self.snapshot()).finish()
    }
}

impl Default for ActivityFeed {
    fn default() -> Self {
        Self::new()
    }
}

const KEY_WINDOW: Duration = Duration::from_millis(500);
const TYPING_WINDOW: Duration = Duration::from_millis(300);

impl ActivityFeed {
    pub fn new() -> Self {
        Self::with_clock(Box::new(Instant::now))
    }

    pub fn with_clock(clock: Clock) -> Self {
        let now = clock();
        Self {
            clock,
            inner: Mutex::new(Inner {
                last_activity_at: now,
                last_key_at: None,
                key_times: VecDeque::new(),
                reported_idle_ms: None,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Note user activity. Clears any idle value reported by an idle backend.
    pub fn record(&self, kind: ActivityKind) {
        let now = (self.clock)();
        let mut g = self.lock();
        g.last_activity_at = now;
        g.reported_idle_ms = None;
        if kind == ActivityKind::Key {
            g.last_key_at = Some(now);
            g.key_times.push_back(now);
            prune(&mut g.key_times, now);
        }
    }

    /// Store the system idle time reported by an idle backend. It overrides the
    /// locally derived value until the next [`record`](Self::record).
    pub fn set_idle_ms(&self, idle_ms: u64) {
        self.lock().reported_idle_ms = Some(idle_ms);
    }

    pub fn idle_ms(&self) -> u64 {
        let now = (self.clock)();
        let g = self.lock();
        match g.reported_idle_ms {
            Some(v) => v,
            None => now.saturating_duration_since(g.last_activity_at).as_millis() as u64,
        }
    }

    pub fn is_typing(&self) -> bool {
        let now = (self.clock)();
        let g = self.lock();
        g.last_key_at.is_some_and(|t| now.saturating_duration_since(t) < TYPING_WINDOW)
    }

    pub fn is_keyboard_burst(&self) -> bool {
        let now = (self.clock)();
        let mut g = self.lock();
        prune(&mut g.key_times, now);
        g.key_times.len() >= 3
    }

    pub fn is_active(&self, threshold_ms: u64) -> bool {
        self.idle_ms() < threshold_ms
    }

    /// 2s while active (< 5s idle), 5s under a minute idle, 10s beyond.
    pub fn recommended_poll_interval(&self) -> Duration {
        recommended_interval_for(self.idle_ms())
    }

    pub fn recommended_poll_interval_sec(&self) -> f64 {
        self.recommended_poll_interval().as_secs_f64()
    }

    pub fn snapshot(&self) -> FeedSnapshot {
        FeedSnapshot {
            idle_ms: self.idle_ms(),
            is_typing: self.is_typing(),
            is_keyboard_burst: self.is_keyboard_burst(),
            recommended_poll_interval_sec: self.recommended_poll_interval_sec(),
        }
    }
}

/// Recommended polling interval for a given idle time (pure; same thresholds as 0.1).
pub fn recommended_interval_for(idle_ms: u64) -> Duration {
    if idle_ms < 5_000 {
        Duration::from_secs(2)
    } else if idle_ms < 60_000 {
        Duration::from_secs(5)
    } else {
        Duration::from_secs(10)
    }
}

fn prune(keys: &mut VecDeque<Instant>, now: Instant) {
    while keys.front().is_some_and(|t| now.saturating_duration_since(*t) > KEY_WINDOW) {
        keys.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn fake() -> (ActivityFeed, Arc<AtomicU64>) {
        let base = Instant::now();
        let offset = Arc::new(AtomicU64::new(0));
        let o = offset.clone();
        let feed = ActivityFeed::with_clock(Box::new(move || base + Duration::from_millis(o.load(Ordering::SeqCst))));
        (feed, offset)
    }

    #[test]
    fn interval_thresholds_match_1x() {
        let (feed, t) = fake();
        assert_eq!(feed.recommended_poll_interval_sec(), 2.0);
        t.store(4_999, Ordering::SeqCst);
        assert_eq!(feed.recommended_poll_interval_sec(), 2.0);
        t.store(5_000, Ordering::SeqCst);
        assert_eq!(feed.recommended_poll_interval_sec(), 5.0);
        t.store(60_000, Ordering::SeqCst);
        assert_eq!(feed.recommended_poll_interval_sec(), 10.0);
        feed.record(ActivityKind::Focus);
        assert_eq!(feed.idle_ms(), 0);
        assert_eq!(feed.recommended_poll_interval_sec(), 2.0);
    }

    #[test]
    fn reported_idle_overrides_until_activity() {
        let (feed, t) = fake();
        feed.set_idle_ms(120_000);
        assert_eq!(feed.idle_ms(), 120_000);
        assert_eq!(feed.recommended_poll_interval_sec(), 10.0);
        t.store(1_000, Ordering::SeqCst);
        assert_eq!(feed.idle_ms(), 120_000);
        feed.record(ActivityKind::Mouse);
        assert_eq!(feed.idle_ms(), 0);
        assert!(!feed.is_active(0));
        assert!(feed.is_active(1));
    }

    #[test]
    fn typing_and_burst() {
        let (feed, t) = fake();
        assert!(!feed.is_typing());
        feed.record(ActivityKind::Key);
        t.store(100, Ordering::SeqCst);
        feed.record(ActivityKind::Key);
        t.store(200, Ordering::SeqCst);
        feed.record(ActivityKind::Key);
        assert!(feed.is_typing());
        assert!(feed.is_keyboard_burst());
        t.store(800, Ordering::SeqCst);
        assert!(!feed.is_typing());
        assert!(!feed.is_keyboard_burst());
        let snap = feed.snapshot();
        assert_eq!(snap.idle_ms, 600);
    }

    #[test]
    fn kind_parsing_rejects_unknown() {
        assert_eq!("key".parse::<ActivityKind>(), Ok(ActivityKind::Key));
        assert!("scroll".parse::<ActivityKind>().is_err());
    }
}
