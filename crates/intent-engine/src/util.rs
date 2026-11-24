//! Small deterministic helpers shared by the engine modules.

use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use futures::future::{Either, select};
use sha2::{Digest, Sha256};

/// Insertion-ordered multiset with Python `Counter` semantics for "most common":
/// the highest count wins and ties go to the value seen first.
///
/// The current leader is maintained incrementally, so adding a value is O(1).
#[derive(Debug, Clone, Default)]
pub struct OrderedCounter {
    order: Vec<String>,
    counts: HashMap<String, (usize, usize)>, // value -> (count, first_seen_index)
    leader: Option<usize>,                   // index into `order`
}

impl OrderedCounter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, value: &str) {
        self.add_n(value, 1);
    }

    pub fn add_n(&mut self, value: &str, n: usize) {
        if n == 0 {
            return;
        }
        let next_index = self.order.len();
        let entry = self.counts.entry(value.to_owned()).or_insert_with(|| (0, next_index));
        if entry.1 == next_index && entry.0 == 0 {
            self.order.push(value.to_owned());
        }
        entry.0 += n;
        let (count, first) = *entry;
        match self.leader {
            None => self.leader = Some(first),
            Some(leader) => {
                let leader_count = self.counts[&self.order[leader]].0;
                if count > leader_count || (count == leader_count && first < leader) {
                    self.leader = Some(first);
                }
            }
        }
    }

    /// Merge `other` after `self` (its unseen values keep their relative order).
    pub fn merge(&mut self, other: &OrderedCounter) {
        for value in &other.order {
            self.add_n(value, other.counts[value].0);
        }
    }

    pub fn most_common(&self) -> Option<(&str, usize)> {
        self.leader.map(|index| {
            let value = self.order[index].as_str();
            (value, self.counts[value].0)
        })
    }

    pub fn top(&self) -> Option<&str> {
        self.most_common().map(|(value, _)| value)
    }

    pub fn count(&self, value: &str) -> usize {
        self.counts.get(value).map_or(0, |(count, _)| *count)
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn contains(&self, value: &str) -> bool {
        self.counts.contains_key(value)
    }

    /// Distinct values in first-seen order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.order.iter().map(String::as_str)
    }

    /// Distinct values ordered by count (desc), then first-seen.
    pub fn by_frequency(&self) -> Vec<(&str, usize)> {
        let mut items: Vec<(&str, usize, usize)> =
            self.order.iter().map(|v| (v.as_str(), self.counts[v].0, self.counts[v].1)).collect();
        items.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)));
        items.into_iter().map(|(v, c, _)| (v, c)).collect()
    }

    pub fn intersects(&self, other: &OrderedCounter) -> bool {
        let (small, large) = if self.len() <= other.len() { (self, other) } else { (other, self) };
        small.order.iter().any(|value| large.contains(value))
    }
}

impl<'a> FromIterator<&'a str> for OrderedCounter {
    fn from_iter<I: IntoIterator<Item = &'a str>>(iter: I) -> Self {
        let mut counter = OrderedCounter::new();
        for value in iter {
            counter.add(value);
        }
        counter
    }
}

/// First-seen most common value of an iterator (Python `Counter` + `max` semantics).
pub fn most_common<'a, I: IntoIterator<Item = &'a str>>(values: I) -> Option<String> {
    values.into_iter().collect::<OrderedCounter>().top().map(str::to_owned)
}

/// Order-preserving de-duplication.
pub fn unique<T: PartialEq + Clone, I: IntoIterator<Item = T>>(values: I) -> Vec<T> {
    let mut out: Vec<T> = Vec::new();
    for value in values {
        if !out.contains(&value) {
            out.push(value);
        }
    }
    out
}

/// First 16 hex chars of SHA-256.
pub fn sha256_hex16(data: &[u8]) -> String {
    hex16(&Sha256::digest(data))
}

pub fn hex16(digest: &[u8]) -> String {
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Final path segment with `\` treated as `/`.
pub fn basename(path: &str) -> Option<&str> {
    let trimmed = path.rsplit(['/', '\\']).next().unwrap_or("");
    (!trimmed.is_empty()).then_some(trimmed)
}

/// Last path segment after trimming trailing separators (`_final_segment` in 0.1).
pub fn final_segment(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let normalized = normalized.trim_end_matches('/');
    if normalized.is_empty() {
        return None;
    }
    normalized.rsplit('/').next().filter(|s| !s.is_empty()).map(str::to_owned)
}

/// Parent directory (`_parent_path` in 0.1 enrich).
pub fn parent_path(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let normalized = normalized.trim_end_matches('/');
    let (parent, _) = normalized.rsplit_once('/')?;
    (!parent.is_empty()).then(|| parent.to_owned())
}

/// Truncate to at most `max_chars` characters (char-boundary safe).
pub fn truncate_chars(value: &str, max_chars: usize) -> &str {
    match value.char_indices().nth(max_chars) {
        Some((index, _)) => &value[..index],
        None => value,
    }
}

/// Collapse every whitespace run to one space and trim.
pub fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Run `fut` with a deadline; `None` on timeout. Runtime-agnostic.
pub async fn with_timeout<F: Future>(fut: F, timeout: Duration) -> Option<F::Output> {
    let fut = std::pin::pin!(fut);
    let delay = futures_timer::Delay::new(timeout);
    match select(fut, delay).await {
        Either::Left((output, _)) => Some(output),
        Either::Right(((), _)) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_ties_go_to_first_seen() {
        let c: OrderedCounter = ["b", "a", "a", "b", "c"].into_iter().collect();
        assert_eq!(c.most_common(), Some(("b", 2)));
        let c: OrderedCounter = ["a", "b", "b"].into_iter().collect();
        assert_eq!(c.top(), Some("b"));
        assert_eq!(c.by_frequency(), vec![("b", 2), ("a", 1)]);
    }

    #[test]
    fn counter_merge_keeps_order_and_leader() {
        let mut left: OrderedCounter = ["x", "y"].into_iter().collect();
        let right: OrderedCounter = ["y", "z", "z"].into_iter().collect();
        left.merge(&right);
        assert_eq!(left.keys().collect::<Vec<_>>(), vec!["x", "y", "z"]);
        assert_eq!(left.most_common(), Some(("y", 2)));
        assert!(left.intersects(&right));
    }

    #[test]
    fn path_helpers() {
        assert_eq!(basename("/a/b/c.rs"), Some("c.rs"));
        assert_eq!(basename("C:\\x\\y.py"), Some("y.py"));
        assert_eq!(basename("/a/"), None);
        assert_eq!(final_segment("/home/u/proj/"), Some("proj".into()));
        assert_eq!(final_segment("/"), None);
        assert_eq!(parent_path("/home/u/a.py"), Some("/home/u".into()));
        assert_eq!(parent_path("a.py"), None);
        assert_eq!(parent_path("/a.py"), None);
    }

    #[test]
    fn misc() {
        assert_eq!(unique(vec![1, 2, 1, 3]), vec![1, 2, 3]);
        assert_eq!(truncate_chars("héllo", 2), "hé");
        assert_eq!(collapse_whitespace("  a \n b  "), "a b");
        assert_eq!(sha256_hex16(b"abc"), "ba7816bf8f01cfea");
    }

    #[tokio::test]
    async fn timeout_helper() {
        assert_eq!(with_timeout(async { 1 }, Duration::from_millis(50)).await, Some(1));
        let slow = futures_timer::Delay::new(Duration::from_millis(200));
        assert!(with_timeout(slow, Duration::from_millis(10)).await.is_none());
    }
}
