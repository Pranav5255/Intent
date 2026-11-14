//! Embedders: an OpenAI-compatible `/v1/embeddings` client (Ollama, llama.cpp, LM Studio,
//! OpenAI) and an always-available offline [`HashingEmbedder`].

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::client::{codes, http_error, shared_http, transport_error};
use crate::traits::{Embedder, LlmError};

/// Cosine similarity; 0 for empty or zero vectors.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 { 0.0 } else { dot / (na.sqrt() * nb.sqrt()) }
}

// ---------------------------------------------------------------------------
// Offline feature hashing
// ---------------------------------------------------------------------------

pub const HASHING_DIMS: usize = 256;

/// Deterministic feature hashing of word unigrams + bigrams with sublinear tf,
/// signed buckets and L2 normalisation. Needs no model, so correlation works offline.
#[derive(Debug, Clone)]
pub struct HashingEmbedder {
    dims: usize,
}

impl Default for HashingEmbedder {
    fn default() -> Self {
        Self { dims: HASHING_DIMS }
    }
}

impl HashingEmbedder {
    pub fn new(dims: usize) -> Self {
        Self { dims: dims.max(8) }
    }

    /// Lowercased word tokens; splits on punctuation, path separators and camelCase.
    pub fn tokenize(text: &str) -> Vec<String> {
        let mut tokens = Vec::new();
        let mut current = String::new();
        let mut prev_lower = false;
        for ch in text.chars() {
            if ch.is_alphanumeric() {
                if ch.is_uppercase() && prev_lower && !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
                prev_lower = ch.is_lowercase() || ch.is_numeric();
                current.extend(ch.to_lowercase());
            } else {
                prev_lower = false;
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
        }
        if !current.is_empty() {
            tokens.push(current);
        }
        tokens.retain(|t| t.chars().count() > 1 || t.chars().all(|c| c.is_numeric()));
        tokens
    }

    pub fn embed_one(&self, text: &str) -> Vec<f32> {
        let tokens = Self::tokenize(text);
        let mut counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        for t in &tokens {
            *counts.entry(t.clone()).or_default() += 1;
        }
        for pair in tokens.windows(2) {
            *counts.entry(format!("{} {}", pair[0], pair[1])).or_default() += 1;
        }
        let mut v = vec![0.0f32; self.dims];
        for (feature, tf) in counts {
            let h = fnv1a(feature.as_bytes());
            let idx = (h % self.dims as u64) as usize;
            let sign = if (h >> 63) & 1 == 1 { -1.0 } else { 1.0 };
            // Bigrams weigh a little less than unigrams.
            let weight = if feature.contains(' ') { 0.7 } else { 1.0 };
            v[idx] += sign * weight * (1.0 + (tf as f32).ln());
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            v.iter_mut().for_each(|x| *x /= norm);
        }
        v
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[async_trait]
impl Embedder for HashingEmbedder {
    fn cache_identity(&self) -> String {
        format!("hashing-fnv1a-uni-bi-v1:{}", self.dims)
    }

    fn dims(&self) -> usize {
        self.dims
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LlmError> {
        Ok(texts.iter().map(|t| self.embed_one(t)).collect())
    }
}

// ---------------------------------------------------------------------------
// OpenAI-compatible /v1/embeddings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct OpenAiEmbedderConfig {
    pub provider: String,
    /// Base URL including `/v1`.
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub batch_size: usize,
    pub timeout: Duration,
}

pub struct OpenAiEmbedder {
    config: OpenAiEmbedderConfig,
    http: reqwest::Client,
    dims: AtomicUsize,
}

/// Known output sizes so `dims()` is right before the first call.
pub fn known_embedding_dims(model: &str) -> Option<usize> {
    let m = model.to_lowercase();
    let m = m.split(':').next().unwrap_or_default();
    Some(match m {
        "nomic-embed-text" | "nomic-embed-text-v1.5" => 768,
        "all-minilm" | "all-minilm-l6-v2" => 384,
        "mxbai-embed-large" => 1024,
        "bge-m3" => 1024,
        "snowflake-arctic-embed" => 1024,
        "text-embedding-3-small" => 1536,
        "text-embedding-3-large" => 3072,
        _ => return None,
    })
}

impl OpenAiEmbedder {
    pub fn new(mut config: OpenAiEmbedderConfig) -> Self {
        config.base_url = config.base_url.trim().trim_end_matches('/').to_owned();
        config.batch_size = config.batch_size.max(1);
        let dims = known_embedding_dims(&config.model).unwrap_or(0);
        Self { config, http: shared_http(), dims: AtomicUsize::new(dims) }
    }

    async fn embed_batch(&self, batch: &[String]) -> Result<Vec<Vec<f32>>, LlmError> {
        let provider = &self.config.provider;
        let mut req = self
            .http
            .post(format!("{}/embeddings", self.config.base_url))
            .timeout(self.config.timeout)
            .json(&json!({"model": self.config.model, "input": batch}));
        if let Some(key) = self.config.api_key.as_deref().filter(|k| !k.is_empty()) {
            req = req.bearer_auth(key);
        }
        let response = req.send().await.map_err(|e| transport_error(provider, e))?;
        if !response.status().is_success() {
            return Err(http_error(provider, response).await);
        }
        let body: Value = response
            .json()
            .await
            .map_err(|_| LlmError::with_code("embeddings response was not JSON", codes::BAD_RESPONSE))?;
        let data = body
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| LlmError::with_code("embeddings response had no data", codes::BAD_RESPONSE))?;
        let mut out: Vec<(usize, Vec<f32>)> = Vec::with_capacity(data.len());
        for (i, item) in data.iter().enumerate() {
            let index = item.get("index").and_then(Value::as_u64).map(|x| x as usize).unwrap_or(i);
            let vector: Vec<f32> = item
                .get("embedding")
                .and_then(Value::as_array)
                .ok_or_else(|| LlmError::with_code("embedding missing", codes::BAD_RESPONSE))?
                .iter()
                .map(|x| x.as_f64().unwrap_or(0.0) as f32)
                .collect();
            out.push((index, vector));
        }
        if out.len() != batch.len() {
            return Err(LlmError::with_code("embedding count mismatch", codes::BAD_RESPONSE));
        }
        out.sort_by_key(|(i, _)| *i);
        Ok(out.into_iter().map(|(_, v)| v).collect())
    }
}

#[async_trait]
impl Embedder for OpenAiEmbedder {
    fn cache_identity(&self) -> String {
        format!("{}:{}@{}", self.config.provider, self.config.model, self.config.base_url)
    }

    fn dims(&self) -> usize {
        self.dims.load(Ordering::Relaxed)
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LlmError> {
        let mut out = Vec::with_capacity(texts.len());
        for batch in texts.chunks(self.config.batch_size) {
            out.extend(self.embed_batch(batch).await?);
        }
        if let Some(first) = out.first() {
            self.dims.store(first.len(), Ordering::Relaxed);
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Model2Vec static embeddings (feature `model2vec`)
// ---------------------------------------------------------------------------

/// Model2Vec static embeddings (e.g. `minishlab/potion-base-8M`) loaded from a local
/// directory containing `tokenizer.json`, `model.safetensors` and `config.json`.
/// Never downloads anything (`local-only`).
#[cfg(feature = "model2vec")]
pub struct Model2VecEmbedder {
    model: std::sync::Arc<model2vec_rs::model::StaticModel>,
    identity: String,
}

#[cfg(feature = "model2vec")]
impl Model2VecEmbedder {
    pub fn load(dir: &std::path::Path) -> Result<Self, LlmError> {
        let model = model2vec_rs::model::StaticModel::from_pretrained(dir, None, Some(true), None)
            .map_err(|e| LlmError::with_code(format!("could not load Model2Vec model: {e}"), codes::CONFIG))?;
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("model2vec");
        let identity = format!("model2vec:{name}:{}", model.dim());
        Ok(Self { model: std::sync::Arc::new(model), identity })
    }
}

#[cfg(feature = "model2vec")]
#[async_trait]
impl Embedder for Model2VecEmbedder {
    fn cache_identity(&self) -> String {
        self.identity.clone()
    }

    fn dims(&self) -> usize {
        self.model.dim()
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LlmError> {
        let model = std::sync::Arc::clone(&self.model);
        let texts = texts.to_vec();
        tokio::task::spawn_blocking(move || model.encode(&texts))
            .await
            .map_err(|_| LlmError::new("Model2Vec encoding task failed"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hashing_is_deterministic_and_normalised() {
        let e = HashingEmbedder::default();
        let a = e.embed(&["Fix flaky pytest in intent_engine/cluster.py".into()]).await.unwrap();
        let b = e.embed(&["Fix flaky pytest in intent_engine/cluster.py".into()]).await.unwrap();
        assert_eq!(a, b);
        assert_eq!(a[0].len(), HASHING_DIMS);
        let norm: f32 = a[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
        assert_eq!(e.dims(), 256);
        // Pinned value guards against accidental algorithm changes (vectors are persisted).
        let pinned = e.embed_one("hello world");
        let nonzero = pinned.iter().filter(|x| **x != 0.0).count();
        assert_eq!(nonzero, 3);
    }

    #[test]
    fn hashing_similarity_sanity() {
        let e = HashingEmbedder::default();
        let a = e.embed_one("terraform plan aws vpc module networking");
        let b = e.embed_one("terraform apply vpc module networking subnets");
        let c = e.embed_one("spotify playlist jazz evening");
        assert!(cosine(&a, &b) > 0.4, "related {}", cosine(&a, &b));
        assert!(cosine(&a, &c) < 0.2, "unrelated {}", cosine(&a, &c));
        assert!(cosine(&a, &b) > cosine(&a, &c));
        assert_eq!(cosine(&e.embed_one(""), &a), 0.0);
    }

    #[test]
    fn tokenizer_splits_paths_and_camel_case() {
        assert_eq!(
            HashingEmbedder::tokenize("src/semanticCluster.rs github.com"),
            vec!["src", "semantic", "cluster", "rs", "github", "com"]
        );
    }

    #[test]
    fn known_dims() {
        assert_eq!(known_embedding_dims("nomic-embed-text:latest"), Some(768));
        assert_eq!(known_embedding_dims("mystery"), None);
    }
}
