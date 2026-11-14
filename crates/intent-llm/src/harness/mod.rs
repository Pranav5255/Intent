//! The structured-output core used by every caller.
//!
//! request → (cache) → limiter → provider with bounded retries (429/5xx, Retry-After,
//! jittered backoff, overall deadline) → tolerant JSON extraction → serde + caller
//! validation → on failure, ONE repair turn carrying the validation error → typed result.

pub mod budget;
pub mod cache;
pub mod extract;
pub mod redact;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::Semaphore;
use tokio::time::Instant;

pub use cache::{CachePersistence, ResponseCache};

use crate::client::{ChatClient, ChatMessage, ClientIdentity, JsonMode, JsonRequest, Repair, ToolSpec, ToolTurn};
use crate::traits::{LlmError, SemanticFallback};
use crate::util::truncate_chars;

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Total provider attempts per request (first try included).
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
    /// Upper bound for a server-supplied Retry-After.
    pub max_retry_after: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(20),
            max_retry_after: Duration::from_secs(60),
        }
    }
}

impl RetryPolicy {
    /// Delay before attempt `attempt + 1`, honouring Retry-After when present.
    pub fn delay(&self, attempt: u32, err: &LlmError) -> Duration {
        if let Some(secs) = err.retry_after_seconds.filter(|s| s.is_finite() && *s >= 0.0) {
            let jitter = Duration::from_millis(fastrand::u64(0..=250));
            return Duration::from_secs_f64(secs).min(self.max_retry_after) + jitter;
        }
        let exp = self.base_delay.saturating_mul(1u32 << attempt.saturating_sub(1).min(16));
        let capped = exp.min(self.max_delay);
        // Equal jitter: half fixed, half random.
        let half = capped / 2;
        half + Duration::from_millis(fastrand::u64(0..=half.as_millis() as u64))
    }
}

#[derive(Debug, Clone)]
pub struct HarnessConfig {
    /// Concurrent in-flight provider requests (default 1 local / 4 cloud).
    pub concurrency: usize,
    pub retry: RetryPolicy,
    pub cache_capacity: usize,
    /// Overall deadline for one structured call, including retries and the repair turn.
    pub default_deadline: Duration,
    /// Send the validation error back once before giving up.
    pub repair: bool,
}

impl HarnessConfig {
    pub fn for_identity(identity: &ClientIdentity) -> Self {
        Self {
            concurrency: if identity.is_local { 1 } else { 4 },
            retry: RetryPolicy::default(),
            cache_capacity: 512,
            default_deadline: if identity.is_local { Duration::from_secs(180) } else { Duration::from_secs(60) },
            repair: true,
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum HarnessError {
    #[error("LLM request timed out")]
    Timeout,
    #[error("{0}")]
    Provider(LlmError),
    #[error("invalid model output: {0}")]
    Invalid(String),
}

impl HarnessError {
    pub fn fallback(&self) -> SemanticFallback {
        match self {
            HarnessError::Timeout => SemanticFallback::Timeout,
            HarnessError::Provider(e) if e.is_timeout() => SemanticFallback::Timeout,
            HarnessError::Provider(_) => SemanticFallback::ProviderUnavailable,
            HarnessError::Invalid(_) => SemanticFallback::InvalidResponse,
        }
    }
}

impl From<HarnessError> for LlmError {
    fn from(e: HarnessError) -> Self {
        match e {
            HarnessError::Provider(e) => e,
            HarnessError::Timeout => LlmError::with_code("LLM request timed out", crate::client::codes::TIMEOUT),
            HarnessError::Invalid(m) => LlmError::with_code(m, crate::client::codes::BAD_RESPONSE),
        }
    }
}

/// One structured request.
pub struct StructuredCall<'a> {
    /// Short task name for tracing and cache namespacing (`semantic`, `label`, ...).
    pub task: &'a str,
    /// Bump when prompts change so cached answers are not reused.
    pub prompt_version: &'a str,
    pub system: String,
    pub user: String,
    pub schema_name: &'a str,
    pub schema: Value,
    pub mode: JsonMode,
    pub max_output_tokens: u32,
    pub deadline: Option<Duration>,
    pub use_cache: bool,
    /// Optional reshaping before deserialization (e.g. wrap a bare object).
    pub normalize: Option<&'a (dyn Fn(Value) -> Value + Send + Sync)>,
}

impl<'a> StructuredCall<'a> {
    pub fn new(
        task: &'a str,
        prompt_version: &'a str,
        system: String,
        user: String,
        schema_name: &'a str,
        schema: Value,
    ) -> Self {
        Self {
            task,
            prompt_version,
            system,
            user,
            schema_name,
            schema,
            mode: JsonMode::Schema,
            max_output_tokens: 2048,
            deadline: None,
            use_cache: true,
            normalize: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Structured<T> {
    pub value: T,
    pub cached: bool,
    pub attempts: u32,
    pub repaired: bool,
    pub latency: Duration,
}

#[derive(Debug, Default)]
pub struct HarnessStats {
    pub calls: AtomicU64,
    pub provider_attempts: AtomicU64,
    pub repairs: AtomicU64,
    pub failures: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StatsSnapshot {
    pub calls: u64,
    pub provider_attempts: u64,
    pub repairs: u64,
    pub failures: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
}

pub struct Harness {
    client: Arc<dyn ChatClient>,
    identity: ClientIdentity,
    limiter: Arc<Semaphore>,
    cache: ResponseCache,
    config: HarnessConfig,
    stats: HarnessStats,
}

impl Harness {
    pub fn new(
        client: Arc<dyn ChatClient>,
        config: HarnessConfig,
        persistence: Option<Arc<dyn CachePersistence>>,
    ) -> Self {
        let identity = client.identity();
        Self {
            limiter: Arc::new(Semaphore::new(config.concurrency.max(1))),
            cache: ResponseCache::new(config.cache_capacity, persistence),
            identity,
            client,
            config,
            stats: HarnessStats::default(),
        }
    }

    /// Defaults derived from the client (local → concurrency 1).
    pub fn for_client(client: Arc<dyn ChatClient>) -> Self {
        let config = HarnessConfig::for_identity(&client.identity());
        Self::new(client, config, None)
    }

    pub fn identity(&self) -> &ClientIdentity {
        &self.identity
    }

    pub fn client(&self) -> &Arc<dyn ChatClient> {
        &self.client
    }

    pub fn config(&self) -> &HarnessConfig {
        &self.config
    }

    pub fn stats(&self) -> StatsSnapshot {
        StatsSnapshot {
            calls: self.stats.calls.load(Ordering::Relaxed),
            provider_attempts: self.stats.provider_attempts.load(Ordering::Relaxed),
            repairs: self.stats.repairs.load(Ordering::Relaxed),
            failures: self.stats.failures.load(Ordering::Relaxed),
            cache_hits: self.cache.hits(),
            cache_misses: self.cache.misses(),
        }
    }

    /// Run a schema-constrained request and return a validated `T`.
    pub async fn structured<T, V>(&self, call: StructuredCall<'_>, validate: V) -> Result<Structured<T>, HarnessError>
    where
        T: DeserializeOwned + Serialize,
        V: Fn(&T) -> Result<(), String>,
    {
        let started = Instant::now();
        self.stats.calls.fetch_add(1, Ordering::Relaxed);
        let system = redact::redact_text(&call.system);
        let user = redact::redact_text(&call.user);
        let schema_text = call.schema.to_string();
        let key = ResponseCache::key(
            &self.identity.key(),
            call.prompt_version,
            &[call.task, call.schema_name, &schema_text, &system, &user],
        );
        if call.use_cache
            && let Some(hit) = self.cache.get(&key)
            && let Ok(value) = serde_json::from_str::<T>(&hit)
            && validate(&value).is_ok()
        {
            tracing::debug!(task = call.task, provider = %self.identity.provider, model = %self.identity.model, cache_hit = true, "llm structured call");
            return Ok(Structured { value, cached: true, attempts: 0, repaired: false, latency: started.elapsed() });
        }

        let deadline = started + call.deadline.unwrap_or(self.config.default_deadline);
        let mut request = JsonRequest::new(system, user, call.schema_name, call.schema.clone());
        request.mode = call.mode;
        request.max_output_tokens = call.max_output_tokens;

        let outcome = async {
            let (text, mut attempts) = self.send_json(&request, deadline).await?;
            let first_error = match parse_validate::<T, _>(&text, call.normalize, &validate) {
                Ok(value) => return Ok((value, attempts, false)),
                Err(e) => e,
            };
            if !self.config.repair {
                return Err(HarnessError::Invalid(first_error));
            }
            tracing::debug!(task = call.task, error = %first_error, "structured output invalid; sending one repair turn");
            self.stats.repairs.fetch_add(1, Ordering::Relaxed);
            request.repair = Some(Repair { previous_output: truncate_chars(&text, 4000).to_owned(), error: first_error });
            let (text, more) = self.send_json(&request, deadline).await?;
            attempts += more;
            parse_validate::<T, _>(&text, call.normalize, &validate)
                .map(|value| (value, attempts, true))
                .map_err(HarnessError::Invalid)
        }
        .await;

        let latency = started.elapsed();
        match outcome {
            Ok((value, attempts, repaired)) => {
                if call.use_cache
                    && let Ok(serialized) = serde_json::to_string(&value)
                {
                    self.cache.put(&key, &serialized);
                }
                tracing::info!(
                    task = call.task, provider = %self.identity.provider, model = %self.identity.model,
                    attempts, repaired, cache_hit = false, latency_ms = latency.as_millis() as u64,
                    "llm structured call"
                );
                Ok(Structured { value, cached: false, attempts, repaired, latency })
            }
            Err(err) => {
                self.stats.failures.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    task = call.task, provider = %self.identity.provider, model = %self.identity.model,
                    latency_ms = latency.as_millis() as u64, error = %err, "llm structured call failed"
                );
                Err(err)
            }
        }
    }

    async fn send_json(&self, request: &JsonRequest, deadline: Instant) -> Result<(String, u32), HarnessError> {
        let client = Arc::clone(&self.client);
        self.with_retry(deadline, move || {
            let client = Arc::clone(&client);
            let request = request.clone();
            async move { client.respond_json(&request).await }
        })
        .await
    }

    /// One tool-calling turn through the limiter, retry policy and deadline.
    pub async fn tools_turn(
        &self,
        system: &str,
        messages: &[ChatMessage],
        tools: &[ToolSpec],
        timeout: Duration,
    ) -> Result<ToolTurn, HarnessError> {
        let deadline = Instant::now() + timeout;
        let system = redact::redact_text(system);
        let messages: Vec<ChatMessage> = messages
            .iter()
            .map(|m| match m {
                ChatMessage::User { content } => ChatMessage::User { content: redact::redact_text(content) },
                other => other.clone(),
            })
            .collect();
        let client = Arc::clone(&self.client);
        let (turn, _) = self
            .with_retry(deadline, || {
                let client = Arc::clone(&client);
                let system = system.clone();
                let messages = messages.clone();
                async move { client.respond_with_tools(&system, &messages, tools).await }
            })
            .await?;
        Ok(turn)
    }

    async fn with_retry<R, F, Fut>(&self, deadline: Instant, mut op: F) -> Result<(R, u32), HarnessError>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = Result<R, LlmError>>,
    {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let permit = tokio::time::timeout_at(deadline, self.limiter.acquire())
                .await
                .map_err(|_| HarnessError::Timeout)?
                .map_err(|_| HarnessError::Provider(LlmError::new("limiter closed")))?;
            self.stats.provider_attempts.fetch_add(1, Ordering::Relaxed);
            let result = tokio::time::timeout_at(deadline, op()).await;
            drop(permit);
            let err = match result {
                Err(_) => return Err(HarnessError::Timeout),
                Ok(Ok(value)) => return Ok((value, attempt)),
                Ok(Err(err)) => err,
            };
            let give_up =
                |err: LlmError| if err.is_timeout() { HarnessError::Timeout } else { HarnessError::Provider(err) };
            if !err.is_retryable() || attempt >= self.config.retry.max_attempts {
                return Err(give_up(err));
            }
            let delay = self.config.retry.delay(attempt, &err);
            if Instant::now() + delay >= deadline {
                // Waiting would blow the deadline; fail now with the real cause.
                return Err(give_up(err));
            }
            tracing::debug!(
                attempt, status = ?err.status_code, retry_after = ?err.retry_after_seconds,
                delay_ms = delay.as_millis() as u64, "retrying llm request"
            );
            tokio::time::sleep(delay).await;
        }
    }
}

fn parse_validate<T, V>(
    text: &str,
    normalize: Option<&(dyn Fn(Value) -> Value + Send + Sync)>,
    validate: &V,
) -> Result<T, String>
where
    T: DeserializeOwned,
    V: Fn(&T) -> Result<(), String>,
{
    let candidates = extract::json_candidates(text);
    if candidates.is_empty() {
        return Err("the reply did not contain a JSON object".into());
    }
    let mut first_error: Option<String> = None;
    for candidate in candidates {
        let candidate = match normalize {
            Some(f) => f(candidate),
            None => candidate,
        };
        match serde_json::from_value::<T>(candidate) {
            Ok(value) => match validate(&value) {
                Ok(()) => return Ok(value),
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            },
            Err(e) => {
                first_error.get_or_insert(format!("schema mismatch: {e}"));
            }
        }
    }
    Err(first_error.unwrap_or_else(|| "invalid JSON".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_is_capped_and_used() {
        let policy = RetryPolicy { max_retry_after: Duration::from_secs(5), ..RetryPolicy::default() };
        let err = LlmError::http(429, "x", Some(100.0), None);
        let d = policy.delay(1, &err);
        assert!(d >= Duration::from_secs(5) && d <= Duration::from_millis(5250));
        let err = LlmError::http(429, "x", Some(1.0), None);
        assert!(policy.delay(1, &err) >= Duration::from_secs(1));
    }

    #[test]
    fn backoff_grows_and_caps() {
        let policy = RetryPolicy::default();
        let err = LlmError::http(503, "x", None, None);
        for attempt in 1..10 {
            let d = policy.delay(attempt, &err);
            assert!(d <= policy.max_delay);
        }
        let d3 = policy.delay(3, &err);
        assert!(d3 >= Duration::from_secs(1)); // half of 2s
    }
}
