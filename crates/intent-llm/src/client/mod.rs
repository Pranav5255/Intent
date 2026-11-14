//! Provider clients behind one [`ChatClient`] trait.
//!
//! Clients are thin transports: they render a provider request, send it once, and
//! return raw model text (for JSON mode) or a [`ToolTurn`]. Retries, extraction,
//! validation, caching and concurrency live in [`crate::harness`].

mod bedrock;
mod gemini;
mod openai;

use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use bedrock::{BedrockAuth, BedrockClient, BedrockConfig};
pub use gemini::{GeminiAuth, GeminiClient, GeminiConfig, ServiceAccountKey};
pub use openai::{OpenAiCompatible, OpenAiConfig};

use crate::traits::LlmError;

/// Who answers: used in cache keys, tracing and provider-specific limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientIdentity {
    /// `local`, `openai`, `groq`, `gemini`, `bedrock`, `mock`, ...
    pub provider: String,
    pub model: String,
    /// Base URL or region; part of the identity so two local servers do not share a cache.
    pub endpoint: String,
    /// True when inference stays on this machine (Ollama, llama.cpp, LM Studio).
    pub is_local: bool,
}

impl ClientIdentity {
    pub fn key(&self) -> String {
        format!("{}:{}@{}", self.provider, self.model, self.endpoint)
    }
}

/// How strictly the provider should constrain output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JsonMode {
    /// Grammar/schema-constrained decoding when the server supports it, else JSON object mode.
    #[default]
    Schema,
    /// Plain JSON object mode; the schema is described in the system prompt.
    Object,
}

/// A previous invalid answer plus the validation error, sent back for one repair attempt.
#[derive(Debug, Clone, PartialEq)]
pub struct Repair {
    pub previous_output: String,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JsonRequest {
    pub system: String,
    pub user: String,
    pub schema_name: String,
    pub schema: Value,
    pub mode: JsonMode,
    pub max_output_tokens: u32,
    pub temperature: f32,
    pub repair: Option<Repair>,
}

impl JsonRequest {
    pub fn new(system: impl Into<String>, user: impl Into<String>, schema_name: &str, schema: Value) -> Self {
        Self {
            system: system.into(),
            user: user.into(),
            schema_name: schema_name.to_owned(),
            schema,
            mode: JsonMode::Schema,
            max_output_tokens: 2048,
            temperature: 0.0,
            repair: None,
        }
    }

    /// Text appended to the system prompt when the provider cannot enforce the schema.
    pub fn schema_instruction(&self) -> String {
        format!(
            "\nReturn only one JSON object (no prose, no markdown) matching this JSON schema: {}",
            serde_json::to_string(&self.schema).unwrap_or_default()
        )
    }

    /// The user-turn text that asks for a repair.
    pub fn repair_instruction(repair: &Repair) -> String {
        format!(
            "Your previous reply was rejected: {}. Reply again with only the corrected JSON object.",
            crate::util::truncate_chars(&repair.error, 400)
        )
    }
}

/// A function the model may call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON schema of the arguments object.
    pub parameters: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
    /// Opaque provider continuation data (Gemini thought signatures) echoed back verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// Provider-neutral conversation turn for tool-calling loops.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum ChatMessage {
    User { content: String },
    Assistant { content: Option<String>, tool_calls: Vec<ToolCall> },
    Tool { call_id: String, name: String, content: String },
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        ChatMessage::User { content: content.into() }
    }
}

/// One model turn: optional text and zero or more tool calls.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ToolTurn {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

/// One transport to one model.
#[async_trait]
pub trait ChatClient: Send + Sync {
    fn identity(&self) -> ClientIdentity;

    /// Ask for a JSON object; returns the raw model text (the harness extracts and validates it).
    async fn respond_json(&self, request: &JsonRequest) -> Result<String, LlmError>;

    /// One tool-calling turn.
    async fn respond_with_tools(
        &self,
        system: &str,
        messages: &[ChatMessage],
        tools: &[ToolSpec],
    ) -> Result<ToolTurn, LlmError>;

    /// Whether native function calling is expected to work. The copilot falls back to a
    /// JSON "action" protocol when this is false (or when a native call is rejected).
    fn supports_native_tools(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------
// Shared HTTP plumbing
// ---------------------------------------------------------------------------

/// Process-wide pooled HTTP client (rustls). Per-request timeouts are set by callers.
pub fn shared_http() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .pool_idle_timeout(Duration::from_secs(90))
                .pool_max_idle_per_host(8)
                .user_agent(concat!("intentd/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("rustls HTTP client builds")
        })
        .clone()
}

/// Stable error codes set by the transports (provider codes are passed through verbatim).
pub mod codes {
    pub const TIMEOUT: &str = "timeout";
    pub const CONNECT: &str = "connect";
    pub const TRANSPORT: &str = "transport";
    pub const BAD_RESPONSE: &str = "bad_response";
    pub const CONFIG: &str = "config";
    pub const TOOLS_UNSUPPORTED: &str = "tools_unsupported";
    pub const SCHEMA_UNSUPPORTED: &str = "schema_unsupported";
}

impl LlmError {
    pub fn with_code(message: impl Into<String>, code: &str) -> Self {
        Self { error_code: Some(code.to_owned()), ..Self::new(message) }
    }

    pub fn http(status: u16, message: impl Into<String>, retry_after: Option<f64>, code: Option<String>) -> Self {
        Self { message: message.into(), status_code: Some(status), retry_after_seconds: retry_after, error_code: code }
    }

    pub fn is_timeout(&self) -> bool {
        self.error_code.as_deref() == Some(codes::TIMEOUT) || self.status_code == Some(408)
    }

    /// 429, 5xx, timeouts and connection failures are worth retrying.
    pub fn is_retryable(&self) -> bool {
        match self.status_code {
            Some(429) | Some(408) => true,
            Some(s) if s >= 500 => true,
            Some(_) => false,
            None => matches!(self.error_code.as_deref(), Some(codes::TIMEOUT | codes::CONNECT | codes::TRANSPORT)),
        }
    }
}

pub(crate) fn transport_error(provider: &str, err: reqwest::Error) -> LlmError {
    let code = if err.is_timeout() {
        codes::TIMEOUT
    } else if err.is_connect() {
        codes::CONNECT
    } else {
        codes::TRANSPORT
    };
    let what = if err.is_timeout() {
        "timed out"
    } else if err.is_connect() {
        "connection failed"
    } else {
        "request failed"
    };
    LlmError::with_code(format!("{provider} {what}"), code)
}

/// Parse `Retry-After` and provider reset headers: seconds, `1m30.5s`, `250ms`, or an HTTP date.
pub fn parse_retry_after(value: &str) -> Option<f64> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    if let Ok(secs) = v.parse::<f64>() {
        return (secs.is_finite() && secs >= 0.0).then_some(secs);
    }
    // Go-style durations used by OpenAI/Groq reset headers and Gemini RetryInfo ("36s").
    let re = regex_duration();
    let mut total = 0.0;
    let mut matched = 0;
    for cap in re.captures_iter(v) {
        let n: f64 = cap[1].parse().ok()?;
        total += match &cap[2] {
            "ms" => n / 1000.0,
            "s" => n,
            "m" => n * 60.0,
            "h" => n * 3600.0,
            _ => return None,
        };
        matched += cap[0].len();
    }
    if matched == v.len() {
        return Some(total);
    }
    parse_http_date_delta(v)
}

fn regex_duration() -> &'static regex::Regex {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"(\d+(?:\.\d+)?)(ms|h|m|s)").expect("valid regex"))
}

/// `Retry-After: Wed, 21 Oct 2015 07:28:00 GMT` → seconds from now.
fn parse_http_date_delta(v: &str) -> Option<f64> {
    let parts: Vec<&str> = v.split_whitespace().collect();
    if parts.len() != 6 {
        return None;
    }
    let day: i64 = parts[1].parse().ok()?;
    let month = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
        .iter()
        .position(|m| *m == parts[2])? as i64
        + 1;
    let year: i64 = parts[3].parse().ok()?;
    let hms: Vec<i64> = parts[4].split(':').filter_map(|p| p.parse().ok()).collect();
    if hms.len() != 3 {
        return None;
    }
    let target = days_from_civil(year, month, day) * 86_400 + hms[0] * 3600 + hms[1] * 60 + hms[2];
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
    Some((target - now).max(0) as f64)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Turn a non-success HTTP response into an [`LlmError`] that keeps status, Retry-After
/// and the provider's error code, without copying the (possibly prompt-echoing) body.
pub(crate) async fn http_error(provider: &str, response: reqwest::Response) -> LlmError {
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let mut retry_after = header("retry-after")
        .or_else(|| header("x-ratelimit-reset-requests"))
        .or_else(|| header("x-ratelimit-reset-tokens"))
        .and_then(|v| parse_retry_after(&v));
    let mut code = header("x-amzn-errortype").map(|v| v.split(':').next().unwrap_or_default().to_owned());
    let body = response.text().await.unwrap_or_default();
    let mut detail = String::new();
    if let Ok(json) = serde_json::from_str::<Value>(&body) {
        let err = json.get("error").unwrap_or(&json);
        if code.is_none() {
            code = err
                .get("code")
                .and_then(|c| c.as_str().map(str::to_owned))
                .or_else(|| err.get("status").and_then(Value::as_str).map(str::to_owned))
                .or_else(|| err.get("type").and_then(Value::as_str).map(str::to_owned))
                .or_else(|| json.get("__type").and_then(Value::as_str).map(str::to_owned));
        }
        if let Some(msg) = err.get("message").and_then(Value::as_str).or_else(|| err.as_str()) {
            detail = crate::util::truncate_chars(msg, 200).to_owned();
        }
        // Gemini: error.details[].retryDelay ("36s").
        if retry_after.is_none()
            && let Some(details) = err.get("details").and_then(Value::as_array)
        {
            retry_after =
                details.iter().find_map(|d| d.get("retryDelay").and_then(Value::as_str)).and_then(parse_retry_after);
        }
    }
    let message = if detail.is_empty() {
        format!("{provider} returned HTTP {status}")
    } else {
        format!("{provider} returned HTTP {status}: {detail}")
    };
    LlmError::http(status, message, retry_after, code)
}

/// Gemini rejects JSON-schema keywords outside its OpenAPI subset; Bedrock rejects
/// numeric/length constraints. Remove `keys` at every nesting level.
pub(crate) fn strip_schema_keys(value: &Value, keys: &[&str]) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(k, _)| !keys.contains(&k.as_str()))
                .map(|(k, v)| {
                    // Never treat property *names* as schema keywords.
                    if k == "properties" {
                        let props = v
                            .as_object()
                            .map(|p| p.iter().map(|(pk, pv)| (pk.clone(), strip_schema_keys(pv, keys))).collect())
                            .unwrap_or_default();
                        (k.clone(), Value::Object(props))
                    } else {
                        (k.clone(), strip_schema_keys(v, keys))
                    }
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(|v| strip_schema_keys(v, keys)).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_formats() {
        assert_eq!(parse_retry_after("7"), Some(7.0));
        assert_eq!(parse_retry_after("1.5"), Some(1.5));
        assert_eq!(parse_retry_after("36s"), Some(36.0));
        assert_eq!(parse_retry_after("1m30.5s"), Some(90.5));
        assert_eq!(parse_retry_after("250ms"), Some(0.25));
        assert_eq!(parse_retry_after("soon"), None);
        assert_eq!(parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT"), Some(0.0));
    }

    #[test]
    fn retryable_classification() {
        assert!(LlmError::http(429, "x", None, None).is_retryable());
        assert!(LlmError::http(503, "x", None, None).is_retryable());
        assert!(!LlmError::http(400, "x", None, None).is_retryable());
        assert!(LlmError::with_code("t", codes::TIMEOUT).is_retryable());
        assert!(!LlmError::new("x").is_retryable());
    }

    #[test]
    fn strip_keeps_property_names() {
        let schema = serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {"pattern": {"type": "string", "pattern": "x"}}
        });
        let out = strip_schema_keys(&schema, &["additionalProperties", "pattern"]);
        assert_eq!(out, serde_json::json!({"type": "object", "properties": {"pattern": {"type": "string"}}}));
    }
}
