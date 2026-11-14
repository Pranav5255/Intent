//! Seams between the deterministic engine and inference.

use async_trait::async_trait;
use intent_types::{NormalizedEvent, SafeIntentFeatures};

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct LlmError {
    pub message: String,
    pub status_code: Option<u16>,
    pub retry_after_seconds: Option<f64>,
    pub error_code: Option<String>,
}

impl LlmError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), status_code: None, retry_after_seconds: None, error_code: None }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LabelResult {
    pub label: String,
    pub summary: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelLevel {
    Cluster,
    Parent,
}

/// Produces a human label for an intent from privacy-safe aggregate features.
#[async_trait]
pub trait LabelProvider: Send + Sync {
    /// Stable identity (provider + model + prompt version) used in cache keys.
    fn cache_identity(&self) -> String;
    async fn label(&self, features: &SafeIntentFeatures, level: LabelLevel) -> Result<LabelResult, LlmError>;
}

/// Why semantic refinement fell back to deterministic clusters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticFallback {
    Disabled,
    ProviderUnavailable,
    Timeout,
    InvalidResponse,
}

/// Per-cluster provenance produced by refinement.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClusterSemantics {
    pub confidence: Option<f64>,
    pub roles: std::collections::BTreeMap<String, i64>,
    pub topic: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticOutcome {
    /// Clusters as lists of indices into the session slice, ordered by first event.
    pub clusters: Vec<Vec<usize>>,
    /// Parallel to `clusters`.
    pub semantics: Vec<ClusterSemantics>,
}

/// Regroups one session's events, given the deterministic clustering as a prior.
#[async_trait]
pub trait SemanticRefiner: Send + Sync {
    fn cache_identity(&self) -> String;
    async fn refine(
        &self,
        session: &[NormalizedEvent],
        deterministic: &[Vec<usize>],
    ) -> Result<SemanticOutcome, SemanticFallback>;
}

/// Text → dense vector. Implementations must be deterministic for a given identity.
#[async_trait]
pub trait Embedder: Send + Sync {
    fn cache_identity(&self) -> String;
    fn dims(&self) -> usize;
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LlmError>;
}
