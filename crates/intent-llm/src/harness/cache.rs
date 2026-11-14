//! Response cache: in-memory LRU plus an optional persistence hook.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::util::{Lru, sha256_hex};

/// Implemented by the store to persist validated responses across restarts.
/// Keys are sha256 hex; values are validated JSON documents.
pub trait CachePersistence: Send + Sync {
    fn load(&self, key: &str) -> Option<String>;
    fn store(&self, key: &str, value: &str);
}

pub struct ResponseCache {
    lru: Mutex<Lru<String, String>>,
    persistence: Option<Arc<dyn CachePersistence>>,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl ResponseCache {
    pub fn new(capacity: usize, persistence: Option<Arc<dyn CachePersistence>>) -> Self {
        Self { lru: Mutex::new(Lru::new(capacity)), persistence, hits: AtomicU64::new(0), misses: AtomicU64::new(0) }
    }

    /// sha256 over (provider identity, model, prompt version, request parts).
    pub fn key(identity: &str, prompt_version: &str, parts: &[&str]) -> String {
        let mut all = vec![identity, prompt_version];
        all.extend_from_slice(parts);
        sha256_hex(&all)
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let hit = self.lru.lock().ok().and_then(|mut l| l.get(&key.to_owned()));
        let hit = hit.or_else(|| {
            let loaded = self.persistence.as_ref()?.load(key)?;
            if let Ok(mut l) = self.lru.lock() {
                l.put(key.to_owned(), loaded.clone());
            }
            Some(loaded)
        });
        match hit {
            Some(v) => {
                self.hits.fetch_add(1, Ordering::Relaxed);
                Some(v)
            }
            None => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    pub fn put(&self, key: &str, value: &str) {
        if let Ok(mut l) = self.lru.lock() {
            l.put(key.to_owned(), value.to_owned());
        }
        if let Some(p) = &self.persistence {
            p.store(key, value);
        }
    }

    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }
}
