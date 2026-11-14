//! Historical-prefix intent prediction (1.x `prediction.py`).
//!
//! 1.x rebuilt the prefix index from the store on every request. Here the index is
//! built once per `(date, store revision)` and cached; callers pass the revision
//! they read and a loader that is only invoked on a cache miss.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use intent_types::{Intent, NormalizedEvent, PredictionResponse, ResumePayload};

pub type Prefix = (String, String, String);

/// Prefix of the last three events: (first family, second category, final command
/// family or first project path).
pub fn event_prefix(events: &[&NormalizedEvent]) -> Option<Prefix> {
    let [a, b, c] = events else { return None };
    let project = c.entities.project_paths.first().cloned().unwrap_or_default();
    Some((a.family.as_str().to_owned(), b.category.clone(), c.entities.command_family.clone().unwrap_or(project)))
}

#[derive(Debug, Clone, PartialEq)]
struct Candidate {
    id: String,
    label: String,
    start_ts: i64,
    end_ts: i64,
    resume_payload: ResumePayload,
}

/// Prefix → intents that ended with it.
#[derive(Debug, Default)]
pub struct PrefixIndex {
    entries: HashMap<Prefix, Vec<Candidate>>,
}

impl PrefixIndex {
    pub fn build(roots: &[Intent]) -> Self {
        let mut entries: HashMap<Prefix, Vec<Candidate>> = HashMap::new();
        for root in roots {
            for intent in root.walk() {
                if let Some(prefix) = &intent.prefix {
                    entries.entry(prefix.clone()).or_default().push(Candidate {
                        id: intent.id.clone(),
                        label: intent.label.clone(),
                        start_ts: intent.start_ts,
                        end_ts: intent.end_ts,
                        resume_payload: intent.resume_payload.clone(),
                    });
                }
            }
        }
        Self { entries }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Needs ≥3 recent events and ≥2 historical matches; most recent match wins.
    pub fn predict(&self, recent: &[NormalizedEvent]) -> Option<PredictionResponse> {
        if recent.len() < 3 {
            return None;
        }
        let tail: Vec<&NormalizedEvent> = recent[recent.len() - 3..].iter().collect();
        let matches = self.entries.get(&event_prefix(&tail)?)?;
        if matches.len() < 2 {
            return None;
        }
        let best = matches.iter().max_by(|a, b| (a.end_ts, a.start_ts, &a.id).cmp(&(b.end_ts, b.start_ts, &b.id)))?;
        Some(PredictionResponse {
            predicted_label: best.label.clone(),
            confidence: (0.7 + 0.1 * (matches.len() as f64 - 2.0)).min(0.95),
            resume_payload: best.resume_payload.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexKey {
    pub date: String,
    pub revision: i64,
}

/// Caches one [`PrefixIndex`] keyed by `(date, store revision)`.
#[derive(Debug, Default)]
pub struct PredictionEngine {
    cache: Mutex<Option<(IndexKey, Arc<PrefixIndex>)>>,
}

impl PredictionEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Cached index for `key`, calling `load` (the day's root intents) only on a miss.
    pub fn index<F: FnOnce() -> Vec<Intent>>(&self, key: IndexKey, load: F) -> Arc<PrefixIndex> {
        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((cached_key, index)) = guard.as_ref()
            && *cached_key == key
        {
            return Arc::clone(index);
        }
        let index = Arc::new(PrefixIndex::build(&load()));
        *guard = Some((key, Arc::clone(&index)));
        index
    }

    pub fn predict<F: FnOnce() -> Vec<Intent>>(
        &self,
        key: IndexKey,
        load: F,
        recent: &[NormalizedEvent],
    ) -> Option<PredictionResponse> {
        self.index(key, load).predict(recent)
    }

    pub fn invalidate(&self) {
        *self.cache.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{cmd, edit, intent, seq};
    use std::cell::Cell;

    fn with_prefix(id: &str, end: i64, label: &str, prefix: &Prefix) -> Intent {
        let mut i = intent(id, label, end - 10, end);
        i.prefix = Some(prefix.clone());
        i
    }

    #[test]
    fn predicts_from_two_matches_and_caches_by_revision() {
        let recent = seq(vec![edit("a", 0, "/p/a.py", "/p"), edit("b", 1, "/p/a.py", "/p"), cmd("c", 2, "pytest", "/p", 1)]);
        let tail: Vec<&NormalizedEvent> = recent.iter().collect();
        let prefix = event_prefix(&tail).unwrap();
        assert_eq!(prefix, ("editor".into(), "document_change".into(), "pytest".into()));

        let mut root = with_prefix("r", 100, "Run Pytest", &prefix);
        root.children = vec![with_prefix("c1", 50, "Older", &prefix), with_prefix("c2", 200, "Newest", &prefix)];
        let loads = Cell::new(0);
        let engine = PredictionEngine::new();
        let key = IndexKey { date: "d".into(), revision: 1 };
        let load = || {
            loads.set(loads.get() + 1);
            vec![root.clone()]
        };
        let p = engine.predict(key.clone(), load, &recent).unwrap();
        assert_eq!(p.predicted_label, "Newest");
        assert!((p.confidence - 0.8).abs() < 1e-9);
        engine.predict(key.clone(), || panic!("must use cache"), &recent).unwrap();
        assert_eq!(loads.get(), 1);
        let p = engine.predict(IndexKey { revision: 2, ..key }, Vec::new, &recent);
        assert!(p.is_none());
    }

    #[test]
    fn needs_three_events_and_two_matches() {
        let idx = PrefixIndex::build(&[]);
        assert!(idx.is_empty());
        assert!(idx.predict(&seq(vec![edit("a", 0, "/p/a.py", "/p")])).is_none());
    }
}
