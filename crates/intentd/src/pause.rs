//! Capture pause, persisted in store metadata.
//!
//! 0.1 kept the pause flag in process memory, so a crash or restart silently resumed
//! capture. Here the flag and its optional resume time live in the database and are
//! re-read on start; an expired timed pause counts as resumed immediately (ingest
//! checks the deadline itself) and the housekeeping loop persists the resume.

use std::sync::RwLock;

use intent_store::{Store, now_ts};
use serde::{Deserialize, Serialize};

const KEY: &str = "capture_pause";
/// Upper bound on a timed pause (one week).
pub const MAX_PAUSE_SECONDS: i64 = 7 * 86_400;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PauseRecord {
    pub paused: bool,
    /// Unix seconds when capture resumes automatically (`None` = until resumed).
    pub until: Option<i64>,
    /// When the pause started.
    pub since: Option<i64>,
}

impl PauseRecord {
    /// The record as seen at `now` (an expired timed pause is not paused).
    pub fn at(self, now: i64) -> PauseRecord {
        match self.until {
            Some(until) if self.paused && until <= now => PauseRecord::default(),
            _ => self,
        }
    }
}

#[derive(Debug, Default)]
pub struct Pause {
    record: RwLock<PauseRecord>,
}

impl Pause {
    pub fn load(store: &Store) -> Self {
        let record = store
            .get_metadata(KEY)
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str::<PauseRecord>(&raw).ok())
            .unwrap_or_default();
        Self { record: RwLock::new(record) }
    }

    pub fn current(&self) -> PauseRecord {
        self.record.read().unwrap_or_else(|e| e.into_inner()).at(now_ts())
    }

    pub fn is_paused(&self) -> bool {
        self.current().paused
    }

    /// Pause (optionally for `duration_seconds`) or resume, and persist.
    pub fn set(&self, store: &Store, paused: bool, duration_seconds: Option<i64>) -> intent_store::Result<PauseRecord> {
        let now = now_ts();
        let record = if paused {
            PauseRecord {
                paused: true,
                until: duration_seconds.map(|s| now + s.clamp(1, MAX_PAUSE_SECONDS)),
                since: Some(now),
            }
        } else {
            PauseRecord::default()
        };
        store.set_metadata(KEY, &serde_json::to_string(&record)?)?;
        *self.record.write().unwrap_or_else(|e| e.into_inner()) = record;
        Ok(record)
    }

    /// Persist the automatic resume of an expired timed pause. Returns true if it fired.
    pub fn expire_if_due(&self, store: &Store) -> bool {
        let stored = *self.record.read().unwrap_or_else(|e| e.into_inner());
        if stored.paused && !stored.at(now_ts()).paused {
            if let Err(err) = self.set(store, false, None) {
                tracing::warn!(error = %err, "could not persist automatic capture resume");
                return false;
            }
            tracing::info!("timed capture pause expired; capture resumed");
            return true;
        }
        false
    }
}
