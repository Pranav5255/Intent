//! Embedding-assisted correlation (new in 2.0, works without any LLM).
//!
//! Adjacent clusters of one session are merged when the cosine similarity of their
//! label-free *text profiles* (domains, file names, project, context terms) reaches
//! a threshold and the workspace guard allows it. With an [`Embedder`] the profiles
//! are embedded; without one a deterministic, locally computed hashed bag-of-words
//! TF-IDF vector is used.

use std::collections::{BTreeMap, HashMap};

use intent_llm::Embedder;
use intent_types::{NormalizedEvent, PipelineWarning, WarningLevel};

use crate::util::{final_segment, unique};

pub const CORRELATE_POLICY_VERSION: &str = "correlate-v1";

#[derive(Debug, Clone, PartialEq)]
pub struct CorrelateConfig {
    pub enabled: bool,
    /// Cosine threshold for the built-in hashed TF-IDF vectors.
    pub hashed_threshold: f32,
    /// Cosine threshold for embedder vectors (dense embeddings sit higher on average).
    pub embedding_threshold: f32,
    /// Dimensions of the hashed vector.
    pub hashed_dims: usize,
}

impl Default for CorrelateConfig {
    fn default() -> Self {
        Self { enabled: true, hashed_threshold: 0.6, embedding_threshold: 0.85, hashed_dims: 512 }
    }
}

impl CorrelateConfig {
    pub fn disabled() -> Self {
        Self { enabled: false, ..Self::default() }
    }

    /// Cache identity: changes whenever the merge behaviour can change.
    pub fn identity(&self, embedder: Option<&dyn Embedder>) -> String {
        match embedder {
            Some(e) => {
                format!("{CORRELATE_POLICY_VERSION}:embed:{}:{:.3}", e.cache_identity(), self.embedding_threshold)
            }
            None => {
                format!("{CORRELATE_POLICY_VERSION}:hashed-tfidf:{}:{:.3}", self.hashed_dims, self.hashed_threshold)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorMode {
    HashedTfIdf,
    Embedder,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CorrelateOutcome {
    pub clusters: Vec<Vec<usize>>,
    pub merges: usize,
    pub mode: VectorMode,
    pub warnings: Vec<PipelineWarning>,
}

/// Label-free profile of one cluster: weighted tokens plus the project paths used by
/// the workspace guard.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextProfile {
    pub tokens: BTreeMap<String, u32>,
    pub project_paths: Vec<String>,
}

impl TextProfile {
    pub fn of<'a, I: IntoIterator<Item = &'a NormalizedEvent>>(events: I) -> Self {
        let mut profile = TextProfile::default();
        let mut bump = |t: String| *profile.tokens.entry(t).or_insert(0) += 1;
        let mut paths = Vec::new();
        for event in events {
            let e = &event.entities;
            if let Some(domain) = &e.domain {
                let domain = domain.to_lowercase();
                let parts: Vec<&str> = domain.split('.').collect();
                if parts.len() >= 2 {
                    bump(format!("site:{}", parts[parts.len() - 2..].join(".")));
                }
                bump(format!("domain:{domain}"));
            }
            if let Some(name) = &e.file_name {
                bump(format!("file:{}", name.to_lowercase()));
                for part in name.to_lowercase().split(|c: char| !c.is_ascii_alphanumeric()) {
                    if part.len() >= 3 {
                        bump(format!("term:{part}"));
                    }
                }
            }
            for path in &e.project_paths {
                if let Some(seg) = final_segment(path) {
                    bump(format!("project:{}", seg.to_lowercase()));
                }
                paths.push(path.clone());
            }
            for term in &e.context_terms {
                bump(format!("term:{term}"));
            }
        }
        profile.project_paths = unique(paths);
        profile
    }

    /// Human-readable rendering for an embedder (still label-free).
    pub fn render(&self) -> String {
        let mut groups: BTreeMap<&str, Vec<(&str, u32)>> = BTreeMap::new();
        for (token, count) in &self.tokens {
            if let Some((kind, value)) = token.split_once(':') {
                groups.entry(kind).or_default().push((value, *count));
            }
        }
        let mut parts = Vec::new();
        for (kind, mut values) in groups {
            values.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            let list: Vec<&str> = values.iter().take(24).map(|(v, _)| *v).collect();
            parts.push(format!("{kind}: {}", list.join(", ")));
        }
        parts.join("\n")
    }
}

/// Workspace guard: refuse to merge clusters whose project paths are unrelated
/// (neither equal nor nested). Clusters without paths never block.
pub fn workspace_compatible(a: &[String], b: &[String]) -> bool {
    if a.is_empty() || b.is_empty() {
        return true;
    }
    let norm = |p: &str| p.replace('\\', "/").trim_end_matches('/').to_lowercase();
    let nested = |x: &str, y: &str| x == y || x.starts_with(&format!("{y}/")) || y.starts_with(&format!("{x}/"));
    a.iter().any(|x| b.iter().any(|y| nested(&norm(x), &norm(y))))
}

/// Deterministic signed-hash TF-IDF vectors for a set of profiles (IDF over the set).
pub fn hashed_tfidf(profiles: &[TextProfile], dims: usize) -> Vec<Vec<f32>> {
    let dims = dims.max(8);
    let n = profiles.len() as f32;
    let mut df: HashMap<&str, u32> = HashMap::new();
    for p in profiles {
        for token in p.tokens.keys() {
            *df.entry(token.as_str()).or_insert(0) += 1;
        }
    }
    profiles
        .iter()
        .map(|p| {
            let mut v = vec![0f32; dims];
            for (token, &count) in &p.tokens {
                let idf = ((1.0 + n) / (1.0 + df[token.as_str()] as f32)).ln() + 1.0;
                let tf = 1.0 + (count as f32).ln();
                let h = fnv1a(token.as_bytes());
                let sign = if (h >> 63) & 1 == 1 { -1.0 } else { 1.0 };
                v[(h % dims as u64) as usize] += sign * tf * idf;
            }
            normalize(&mut v);
            v
        })
        .collect()
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 { 0.0 } else { dot / (na * nb) }
}

/// Merge adjacent clusters of one session whose text profiles are similar enough.
///
/// `clusters` index into `events` (one session) and are kept in order; merged
/// clusters keep their indices sorted. Falls back to hashed TF-IDF when the embedder
/// errors or returns malformed vectors (with a warning).
pub async fn merge_by_similarity(
    clusters: Vec<Vec<usize>>,
    events: &[NormalizedEvent],
    embedder: Option<&dyn Embedder>,
    config: &CorrelateConfig,
) -> CorrelateOutcome {
    let mut warnings = Vec::new();
    if !config.enabled || clusters.len() < 2 {
        return CorrelateOutcome { clusters, merges: 0, mode: VectorMode::HashedTfIdf, warnings };
    }
    let profiles: Vec<TextProfile> =
        clusters.iter().map(|c| TextProfile::of(c.iter().filter_map(|&i| events.get(i)))).collect();

    let mut mode = VectorMode::HashedTfIdf;
    let mut threshold = config.hashed_threshold;
    let mut vectors = None;
    if let Some(embedder) = embedder {
        let texts: Vec<String> = profiles.iter().map(TextProfile::render).collect();
        match embedder.embed(&texts).await {
            Ok(v) if v.len() == texts.len() && v.iter().all(|x| !x.is_empty() && x.len() == v[0].len()) => {
                vectors = Some(v);
                mode = VectorMode::Embedder;
                threshold = config.embedding_threshold;
            }
            Ok(_) => warnings.push(warn("Correlation embedder returned malformed vectors; used hashed TF-IDF")),
            Err(e) => warnings.push(warn(&format!("Correlation embedder failed ({e}); used hashed TF-IDF"))),
        }
    }
    let vectors = vectors.unwrap_or_else(|| hashed_tfidf(&profiles, config.hashed_dims));

    let mut out: Vec<(Vec<usize>, Vec<f32>, Vec<String>)> = Vec::with_capacity(clusters.len());
    let mut merges = 0;
    for ((indices, vector), profile) in clusters.into_iter().zip(vectors).zip(profiles) {
        if let Some((prev_indices, prev_vector, prev_paths)) = out.last_mut()
            && cosine(prev_vector, &vector) >= threshold
            && workspace_compatible(prev_paths, &profile.project_paths)
        {
            prev_indices.extend(indices);
            prev_indices.sort_unstable();
            let mut sum: Vec<f32> = prev_vector.iter().zip(&vector).map(|(a, b)| a + b).collect();
            normalize(&mut sum);
            *prev_vector = sum;
            prev_paths.extend(profile.project_paths);
            *prev_paths = unique(std::mem::take(prev_paths));
            merges += 1;
            continue;
        }
        out.push((indices, vector, profile.project_paths));
    }
    CorrelateOutcome { clusters: out.into_iter().map(|(c, _, _)| c).collect(), merges, mode, warnings }
}

fn warn(message: &str) -> PipelineWarning {
    PipelineWarning { level: WarningLevel::Warning, message: message.to_owned(), event_id: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{browse, cmd, edit, seq};
    use async_trait::async_trait;
    use intent_llm::LlmError;

    #[test]
    fn guard() {
        let a = vec!["/home/u/p".to_owned()];
        assert!(workspace_compatible(&a, &["/home/u/p/sub".into()]));
        assert!(workspace_compatible(&a, &[]));
        assert!(!workspace_compatible(&a, &["/home/u/q".into()]));
        assert!(!workspace_compatible(&a, &["/home/u/pp".into()]));
    }

    #[test]
    fn hashed_vectors_are_deterministic_and_normalised() {
        let s = seq(vec![edit("a", 0, "/p/sched.rs", "/p"), browse("b", 5, "https://docs.rs/tokio")]);
        let p = vec![TextProfile::of(&s[..1]), TextProfile::of(&s[1..])];
        let v1 = hashed_tfidf(&p, 64);
        let v2 = hashed_tfidf(&p, 64);
        assert_eq!(v1, v2);
        assert!((cosine(&v1[0], &v1[0]) - 1.0).abs() < 1e-5);
        assert!(cosine(&v1[0], &v1[1]) < 0.5);
        assert!(p[0].render().contains("file: sched.rs"));
    }

    #[tokio::test]
    async fn merges_similar_neighbours_only() {
        let s = seq(vec![
            edit("a", 0, "/p/sched.rs", "/p"),
            edit("b", 10, "/p/sched.rs", "/p"),
            edit("c", 20, "/p/sched.rs", "/p"),
            edit("d", 30, "/p/sched.rs", "/p"),
            browse("e", 40, "https://news.ycombinator.com/"),
        ]);
        let clusters = vec![vec![0, 1], vec![2, 3], vec![4]];
        let out = merge_by_similarity(clusters, &s, None, &CorrelateConfig::default()).await;
        assert_eq!(out.clusters, vec![vec![0, 1, 2, 3], vec![4]]);
        assert_eq!(out.merges, 1);
        assert_eq!(out.mode, VectorMode::HashedTfIdf);
    }

    #[tokio::test]
    async fn workspace_guard_blocks_identical_profiles_in_other_repos() {
        let s = seq(vec![cmd("a", 0, "cargo test", "/home/u/p", 0), cmd("b", 10, "cargo test", "/home/u/q", 0)]);
        let out = merge_by_similarity(vec![vec![0], vec![1]], &s, None, &CorrelateConfig::default()).await;
        assert_eq!(out.clusters.len(), 2);
    }

    #[tokio::test]
    async fn disabled_is_identity() {
        let s = seq(vec![edit("a", 0, "/p/x.rs", "/p"), edit("b", 1, "/p/x.rs", "/p")]);
        let out = merge_by_similarity(vec![vec![0], vec![1]], &s, None, &CorrelateConfig::disabled()).await;
        assert_eq!(out.clusters, vec![vec![0], vec![1]]);
    }

    struct FixedEmbedder(Result<Vec<Vec<f32>>, ()>);

    #[async_trait]
    impl Embedder for FixedEmbedder {
        fn cache_identity(&self) -> String {
            "fixed".into()
        }
        fn dims(&self) -> usize {
            2
        }
        async fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, LlmError> {
            self.0.clone().map_err(|()| LlmError::new("boom"))
        }
    }

    #[tokio::test]
    async fn uses_embedder_and_falls_back_on_error() {
        let s = seq(vec![edit("a", 0, "/p/x.rs", "/p"), browse("b", 1, "https://example.com/")]);
        let cfg = CorrelateConfig::default();
        let e = FixedEmbedder(Ok(vec![vec![1.0, 0.0], vec![0.99, 0.05]]));
        let out = merge_by_similarity(vec![vec![0], vec![1]], &s, Some(&e), &cfg).await;
        assert_eq!(out.mode, VectorMode::Embedder);
        assert_eq!(out.clusters, vec![vec![0, 1]]);

        let e = FixedEmbedder(Err(()));
        let out = merge_by_similarity(vec![vec![0], vec![1]], &s, Some(&e), &cfg).await;
        assert_eq!(out.mode, VectorMode::HashedTfIdf);
        assert_eq!(out.clusters.len(), 2);
        assert_eq!(out.warnings.len(), 1);
        assert!(cfg.identity(Some(&e)).contains("embed:fixed"));
        assert!(cfg.identity(None).contains("hashed-tfidf"));
    }
}
