//! Small shared helpers: a bounded LRU map and hashing.

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;

use sha2::{Digest, Sha256};

/// A bounded least-recently-used map. O(log n) per operation; used for the response
/// cache and the copilot conversation memory.
#[derive(Debug)]
pub struct Lru<K, V> {
    capacity: usize,
    tick: u64,
    entries: HashMap<K, (u64, V)>,
    order: BTreeMap<u64, K>,
}

impl<K: Eq + Hash + Clone, V: Clone> Lru<K, V> {
    pub fn new(capacity: usize) -> Self {
        Self { capacity: capacity.max(1), tick: 0, entries: HashMap::new(), order: BTreeMap::new() }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns a clone of the value and marks it most recently used.
    pub fn get(&mut self, key: &K) -> Option<V> {
        self.tick += 1;
        let tick = self.tick;
        let (old_tick, value) = self.entries.get_mut(key)?;
        self.order.remove(old_tick);
        *old_tick = tick;
        self.order.insert(tick, key.clone());
        Some(value.clone())
    }

    pub fn put(&mut self, key: K, value: V) {
        self.tick += 1;
        if let Some((old_tick, _)) = self.entries.remove(&key) {
            self.order.remove(&old_tick);
        }
        self.order.insert(self.tick, key.clone());
        self.entries.insert(key, (self.tick, value));
        while self.entries.len() > self.capacity {
            let Some((&oldest, _)) = self.order.iter().next() else { break };
            if let Some(evicted) = self.order.remove(&oldest) {
                self.entries.remove(&evicted);
            }
        }
    }
}

pub fn sha256_hex(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        // Length-prefix each part so ("ab","c") and ("a","bc") differ.
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Truncate to at most `max` characters on a char boundary.
pub fn truncate_chars(value: &str, max: usize) -> &str {
    match value.char_indices().nth(max) {
        Some((idx, _)) => &value[..idx],
        None => value,
    }
}

/// Collapse all whitespace runs into single spaces.
pub fn squash_ws(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_evicts_least_recent() {
        let mut lru = Lru::new(2);
        lru.put("a", 1);
        lru.put("b", 2);
        assert_eq!(lru.get(&"a"), Some(1));
        lru.put("c", 3);
        assert_eq!(lru.get(&"b"), None);
        assert_eq!(lru.get(&"a"), Some(1));
        assert_eq!(lru.get(&"c"), Some(3));
        assert_eq!(lru.len(), 2);
    }

    #[test]
    fn hash_is_length_prefixed() {
        assert_ne!(sha256_hex(&["ab", "c"]), sha256_hex(&["a", "bc"]));
        assert_eq!(sha256_hex(&["x"]).len(), 64);
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate_chars("héllo", 2), "hé");
        assert_eq!(truncate_chars("hi", 10), "hi");
    }
}
