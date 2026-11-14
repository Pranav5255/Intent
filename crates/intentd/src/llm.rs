//! Inference wiring: the swappable provider bundle, the copilot backend over the
//! store, the LLM reranker for `/resume/select`, and the local-server probe.
//!
//! Everything provider-related lives in one immutable [`LlmRuntime`] held in an
//! `ArcSwap`; `PUT /settings/llm` builds a new runtime and swaps it in atomically
//! (1.x mutated `os.environ` and re-created clients in place).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use intent_engine::PipelineDeps;
use intent_engine::current::CurrentIntentEngine;
use intent_engine::resume_select::{RerankCandidate, Reranker};
use intent_engine::TemplateLabelProvider;
use intent_llm::copilot::tools::DayIntentStats;
use intent_llm::harness::StructuredCall;
use intent_llm::settings::LocalServerProbe;
use intent_llm::{
    Copilot, CopilotBackend, CopilotConfig, Embedder, Harness, IntentStatsReport, LabelProvider, LlmError, LlmSettings,
    Providers,
};
use intent_store::{EmbedderSpec, Store, now_ts};
use intent_types::{CurrentIntent, Intent, SearchHit};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Builds providers from settings; tests inject mocks here.
pub type ProviderFactory = Arc<dyn Fn(&LlmSettings) -> Providers + Send + Sync>;

pub fn default_factory() -> ProviderFactory {
    Arc::new(intent_llm::build_providers)
}

/// One consistent snapshot of the inference configuration.
pub struct LlmRuntime {
    pub settings: LlmSettings,
    pub providers: Providers,
    pub copilot: Option<Arc<Copilot>>,
    pub reranker: Option<Arc<LlmReranker>>,
}

impl LlmRuntime {
    pub fn build(settings: LlmSettings, factory: &ProviderFactory, store: &Store, current: &Arc<CurrentIntentEngine>) -> Self {
        let providers = factory(&settings);
        if let Some(reason) = &providers.unavailable_reason {
            tracing::info!(reason = %reason, "LLM unavailable; using template labels");
        }
        let backend: Arc<dyn CopilotBackend> = Arc::new(StoreBackend {
            store: store.clone(),
            current: Arc::clone(current),
            embedder: Arc::clone(&providers.embedder),
        });
        let copilot = providers.copilot.as_ref().map(|harness| {
            let mut config = CopilotConfig::for_identity(harness.identity());
            config.tool_mode = settings.copilot_tool_mode;
            Arc::new(Copilot::new(Arc::clone(harness), backend, config))
        });
        let reranker = providers.harness.as_ref().map(|h| Arc::new(LlmReranker { harness: Arc::clone(h) }));
        Self { settings, providers, copilot, reranker }
    }

    /// Short description for `/v1/status`.
    pub fn status(&self, probe: Option<&LocalServerProbe>) -> serde_json::Value {
        let s = &self.settings;
        json!({
            "provider": s.provider,
            "model": self.providers.harness.as_ref().map(|h| h.identity().model.clone()),
            "base_url": s.effective_base_url(),
            "is_local": s.is_local(),
            "reachable": probe.map(|p| p.reachable),
            "available_models": probe.map(|p| p.models.clone()).unwrap_or_default(),
            "unavailable_reason": self.providers.unavailable_reason,
            "embedder": self.providers.embedder.cache_identity(),
            "copilot_enabled": self.copilot.is_some(),
            "semantic_enabled": self.providers.semantic.is_some(),
            "harness": self.providers.harness.as_ref().map(|h| {
                let st = h.stats();
                json!({"calls": st.calls, "provider_attempts": st.provider_attempts, "repairs": st.repairs,
                       "failures": st.failures, "cache_hits": st.cache_hits, "cache_misses": st.cache_misses})
            }),
        })
    }

    /// Engine dependencies. When the provider is a local server that is not running,
    /// inference is skipped entirely (template labels, hashed correlation) instead of
    /// paying a connect error per cluster.
    pub fn pipeline_deps(&self, inference_usable: bool) -> PipelineDeps {
        let s = &self.settings;
        let mut deps = match (&self.providers.label, inference_usable) {
            (Some(label), true) => PipelineDeps::new(Arc::clone(label)),
            _ => PipelineDeps::template_only(),
        };
        if inference_usable {
            deps.semantic_refiner = self.providers.semantic.clone();
        }
        if inference_usable || is_offline_embedder(self.providers.embedder.as_ref()) {
            deps.embedder = Some(Arc::clone(&self.providers.embedder));
        }
        // The provider enforces its own deadline; the engine's outer timeout must not
        // be shorter (the engine default of 5s is tuned for cloud models).
        let label_ms = s.label_timeout_ms.filter(|ms| *ms > 0).map(|ms| ms as u64);
        let local = s.is_local();
        deps.config.label_timeout =
            Duration::from_millis(label_ms.unwrap_or(if local { 20_000 } else { 8_000 }) + 2_000);
        deps.config.semantic_timeout = Duration::from_millis(s.semantic_timeout_ms.clamp(1_000, 600_000) as u64);
        if local {
            // One local model serves one request at a time; queueing more only adds latency.
            deps.config.label_concurrency = 1;
            deps.config.session_concurrency = 1;
        }
        deps
    }

    pub fn embedder_spec(&self) -> Option<EmbedderSpec> {
        let e = &self.providers.embedder;
        (e.dims() > 0).then(|| EmbedderSpec { identity: e.cache_identity(), dims: e.dims() })
    }
}

/// The hashing (and model2vec) embedders run in-process and never need a server.
pub fn is_offline_embedder(e: &dyn Embedder) -> bool {
    let id = e.cache_identity();
    id.starts_with("hashing") || id.starts_with("model2vec")
}

// ---------------------------------------------------------------------------
// Copilot backend over the store
// ---------------------------------------------------------------------------

pub struct StoreBackend {
    pub store: Store,
    pub current: Arc<CurrentIntentEngine>,
    pub embedder: Arc<dyn Embedder>,
}

/// Embed one query with a short deadline; `None` if the embedder is slow/unavailable.
pub async fn embed_query(embedder: &dyn Embedder, text: &str) -> Option<Vec<f32>> {
    let texts = [text.to_string()];
    match tokio::time::timeout(Duration::from_secs(3), embedder.embed(&texts)).await {
        Ok(Ok(mut v)) if !v.is_empty() => Some(v.remove(0)),
        _ => None,
    }
}

pub async fn hybrid_search(
    store: &Store,
    embedder: &dyn Embedder,
    query: &str,
    limit: usize,
    date_from: Option<String>,
    date_to: Option<String>,
) -> intent_store::Result<Vec<SearchHit>> {
    let configured = {
        let store = store.clone();
        tokio::task::spawn_blocking(move || store.embedder()).await.ok().and_then(Result::ok).flatten()
    };
    let vector = match configured {
        Some(spec) if spec.identity == embedder.cache_identity() => {
            embed_query(embedder, query).await.filter(|v| v.len() == spec.dims)
        }
        _ => None,
    };
    let store = store.clone();
    let query = query.to_string();
    tokio::task::spawn_blocking(move || {
        store.hybrid_search(&query, vector.as_deref(), limit, date_from.as_deref(), date_to.as_deref())
    })
    .await
    .map_err(|e| intent_store::StoreError::InvalidArgument(e.to_string()))?
}

/// Current intent from the last 30 minutes of capture events (template labels: the
/// copilot asks often and must not queue behind the pipeline on a local model).
pub async fn current_intent(store: &Store, current: &CurrentIntentEngine, provider: &dyn LabelProvider) -> Option<CurrentIntent> {
    let store = store.clone();
    current
        .get_current(now_ts(), provider, move |since| async move {
            tokio::task::spawn_blocking(move || store.events_since(since).ok()).await.ok().flatten()
        })
        .await
}

#[async_trait]
impl CopilotBackend for StoreBackend {
    async fn search_intents(
        &self,
        query: &str,
        limit: usize,
        date_from: Option<&str>,
        date_to: Option<&str>,
    ) -> Result<Vec<SearchHit>, String> {
        hybrid_search(
            &self.store,
            self.embedder.as_ref(),
            query,
            limit,
            date_from.map(str::to_string),
            date_to.map(str::to_string),
        )
        .await
        .map_err(|e| match e {
            intent_store::StoreError::InvalidArgument(m) => m,
            _ => "search failed".into(),
        })
    }

    async fn get_intent(&self, intent_id: &str) -> Result<Option<Intent>, String> {
        let store = self.store.clone();
        let id = intent_id.to_string();
        tokio::task::spawn_blocking(move || store.intent_by_id(&id))
            .await
            .map_err(|_| "lookup failed".to_string())?
            .map_err(|_| "lookup failed".to_string())
    }

    async fn get_current_intent(&self) -> Result<Option<CurrentIntent>, String> {
        Ok(current_intent(&self.store, &self.current, &TemplateLabelProvider).await)
    }

    async fn get_intent_stats(
        &self,
        date_from: &str,
        date_to: &str,
        project: Option<&str>,
    ) -> Result<IntentStatsReport, String> {
        let store = self.store.clone();
        let (from, to, project) = (date_from.to_string(), date_to.to_string(), project.map(str::to_string));
        let report = tokio::task::spawn_blocking(move || store.stats(&from, &to, project.as_deref()))
            .await
            .map_err(|_| "stats failed".to_string())?
            .map_err(|e| match e {
                intent_store::StoreError::InvalidArgument(m) => m,
                _ => "stats failed".into(),
            })?;
        Ok(IntentStatsReport {
            date_from: Some(report.date_from),
            date_to: Some(report.date_to),
            intent_count: report.intent_count,
            total_duration_seconds: report.total_duration_seconds,
            event_count: report.event_count,
            by_date: report
                .by_date
                .into_iter()
                .map(|d| DayIntentStats { date: d.date, intent_count: d.intent_count, duration_seconds: d.duration_seconds })
                .collect(),
        })
    }
}

// ---------------------------------------------------------------------------
// Reranker
// ---------------------------------------------------------------------------

const RERANK_PROMPT_VERSION: &str = "rerank-v1";
const RERANK_SYSTEM: &str = "You order a user's past work sessions by how well they match what the user wants to resume. \
Use only the labels, summaries and tags given. Return every candidate id exactly once, best match first.";

pub struct LlmReranker {
    harness: Arc<Harness>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RerankOutput {
    order: Vec<String>,
}

#[async_trait]
impl Reranker for LlmReranker {
    async fn rerank(&self, query: &str, candidates: &[RerankCandidate]) -> Result<Vec<String>, LlmError> {
        let ids: Vec<&str> = candidates.iter().map(|c| c.intent_id.as_str()).collect();
        let listing: Vec<serde_json::Value> = candidates
            .iter()
            .map(|c| json!({"id": c.intent_id, "label": c.label, "summary": c.summary, "tags": c.tags}))
            .collect();
        let user = json!({"request": query, "candidates": listing}).to_string();
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["order"],
            "properties": {"order": {"type": "array", "items": {"type": "string", "enum": ids},
                                     "minItems": ids.len(), "maxItems": ids.len()}}
        });
        let mut call = StructuredCall::new("rerank", RERANK_PROMPT_VERSION, RERANK_SYSTEM.into(), user, "rerank", schema);
        call.max_output_tokens = 64 * ids.len().max(1) as u32;
        let supplied: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
        let out = self
            .harness
            .structured::<RerankOutput, _>(call, |o| {
                let mut a = o.order.clone();
                let mut b = supplied.clone();
                a.sort();
                b.sort();
                if a == b { Ok(()) } else { Err("order must contain every candidate id exactly once".into()) }
            })
            .await?;
        Ok(out.value.order)
    }
}
