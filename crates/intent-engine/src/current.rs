//! Current-intent inference over the last 30 minutes (1.x `current.py`).
//!
//! The label provider is passed per call (so LLM settings changes apply
//! immediately) and is part of the cache key. The cache is bounded and keyed by
//! wall-clock minute.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;

use intent_llm::{LabelLevel, LabelProvider};
use intent_types::{CurrentIntent, NormalizedEvent, RawEvent};

use crate::cluster::cluster_session;
use crate::labeling::{TemplateLabelProvider, build_safe_cluster_features, label_with_fallback};
use crate::normalize::normalize_events;
use crate::sessionize::{DEFAULT_SESSION_GAP_MINUTES, sessionize};

pub const CURRENT_WINDOW_SECONDS: i64 = 30 * 60;
pub const MIN_CONFIDENCE: f64 = 0.5;
const CACHE_CAPACITY: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CacheKey {
    minute: i64,
    provider: String,
}

#[derive(Debug)]
pub struct CurrentIntentEngine {
    cache: Mutex<VecDeque<(CacheKey, Option<CurrentIntent>)>>,
    fallback: TemplateLabelProvider,
    label_timeout: Duration,
}

impl Default for CurrentIntentEngine {
    fn default() -> Self {
        Self::new(Duration::from_secs(5))
    }
}

/// Confidence from the size of the most recent cluster (1.x thresholds).
pub fn cluster_confidence(event_count: usize) -> f64 {
    match event_count {
        n if n > 5 => 0.8,
        n if n >= 3 => 0.6,
        _ => 0.3,
    }
}

impl CurrentIntentEngine {
    pub fn new(label_timeout: Duration) -> Self {
        Self { cache: Mutex::new(VecDeque::new()), fallback: TemplateLabelProvider, label_timeout }
    }

    /// Current intent at `now_ts`. `load(since_ts)` fetches capture events since the
    /// start of the window and is only called on a cache miss; `None` from it means
    /// the source is unavailable (not cached).
    pub async fn get_current<F, Fut>(&self, now_ts: i64, provider: &dyn LabelProvider, load: F) -> Option<CurrentIntent>
    where
        F: FnOnce(i64) -> Fut,
        Fut: Future<Output = Option<Vec<RawEvent>>>,
    {
        let key = CacheKey { minute: now_ts.div_euclid(60), provider: provider.cache_identity() };
        if let Some(hit) = self.lookup(&key) {
            return hit;
        }
        let since = now_ts - CURRENT_WINDOW_SECONDS;
        let events = load(since).await?;
        let result = self.infer(&events, since, provider).await;
        self.store(key, result.clone());
        result
    }

    /// Uncached inference over `events` (only those at or after `since_ts` count).
    pub async fn infer(&self, events: &[RawEvent], since_ts: i64, provider: &dyn LabelProvider) -> Option<CurrentIntent> {
        let window: Vec<RawEvent> = events.iter().filter(|e| e.ts >= since_ts).cloned().collect();
        let (normalized, _) = normalize_events(&window);
        let last_session = sessionize(&normalized, DEFAULT_SESSION_GAP_MINUTES).pop()?;
        let session = &normalized[last_session];
        let last_cluster = cluster_session(session).clusters.pop()?;
        let cluster: Vec<&NormalizedEvent> = last_cluster.iter().map(|&i| &session[i]).collect();
        let confidence = cluster_confidence(cluster.len());
        if confidence < MIN_CONFIDENCE {
            return None;
        }
        let features = build_safe_cluster_features(&cluster, None, None);
        let (label, _) =
            label_with_fallback(provider, &self.fallback, &features, LabelLevel::Cluster, self.label_timeout).await;
        Some(CurrentIntent { label: label.label, summary: label.summary, confidence, since_ts: cluster[0].ts })
    }

    fn lookup(&self, key: &CacheKey) -> Option<Option<CurrentIntent>> {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    fn store(&self, key: CacheKey, value: Option<CurrentIntent>) {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.retain(|(k, _)| *k != key && k.minute >= key.minute - 1);
        cache.push_back((key, value));
        while cache.len() > CACHE_CAPACITY {
            cache.pop_front();
        }
    }

    pub fn clear(&self) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::raw;
    use async_trait::async_trait;
    use intent_llm::{LabelResult, LlmError};
    use intent_types::SafeIntentFeatures;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn events(now: i64) -> Vec<RawEvent> {
        let mut v = vec![raw("old", now - 3600, "shell", "command", json!({"cmd": "git status", "cwd": "/p"}))];
        for i in 0..6 {
            v.push(raw(&format!("c{i}"), now - 300 + i * 10, "shell", "command", json!({"cmd": "cargo test", "cwd": "/p"})));
        }
        v
    }

    struct Named(&'static str);
    #[async_trait]
    impl LabelProvider for Named {
        fn cache_identity(&self) -> String {
            self.0.into()
        }
        async fn label(&self, _f: &SafeIntentFeatures, _l: LabelLevel) -> Result<LabelResult, LlmError> {
            Ok(LabelResult { label: format!("From {}", self.0), summary: "Did things.".into(), confidence: 0.9 })
        }
    }

    #[tokio::test]
    async fn infers_labels_and_caches_per_minute_and_provider() {
        let engine = CurrentIntentEngine::default();
        let now = 1_000_000;
        let loads = AtomicUsize::new(0);
        let load = |since: i64| {
            loads.fetch_add(1, Ordering::SeqCst);
            assert_eq!(since, now - CURRENT_WINDOW_SECONDS);
            async move { Some(events(now)) }
        };
        let c = engine.get_current(now, &TemplateLabelProvider, load).await.unwrap();
        assert_eq!(c.label, "Run Cargo");
        assert_eq!(c.confidence, 0.8);
        assert_eq!(c.since_ts, now - 300);
        // Same minute + provider: cached.
        let again = engine.get_current(now + 5, &TemplateLabelProvider, |_| async { None }).await;
        assert_eq!(again, Some(c));
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        // Provider change applies immediately.
        let other = engine.get_current(now + 5, &Named("llm"), |_| async move { Some(events(now)) }).await.unwrap();
        assert_eq!(other.label, "From llm");
    }

    #[tokio::test]
    async fn low_confidence_and_unavailable() {
        let engine = CurrentIntentEngine::default();
        let now = 5_000_000;
        let few = vec![raw("a", now - 10, "shell", "command", json!({"cmd": "ls"}))];
        assert!(engine.get_current(now, &TemplateLabelProvider, |_| async move { Some(few) }).await.is_none());
        assert!(engine.get_current(now + 120, &TemplateLabelProvider, |_| async { None }).await.is_none());
        assert!(engine.cache.lock().unwrap().len() <= CACHE_CAPACITY);
    }
}
