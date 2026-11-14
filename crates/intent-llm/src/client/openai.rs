//! OpenAI-compatible Chat Completions transport.
//!
//! One implementation serves Ollama (`/v1`), llama.cpp `llama-server`, LM Studio, vLLM,
//! Groq and OpenAI. Structured output uses `response_format: json_schema` and falls back
//! to `json_object` (with the schema in the system prompt) when a server rejects it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{
    ChatClient, ChatMessage, ClientIdentity, JsonMode, JsonRequest, ToolCall, ToolSpec, ToolTurn, codes, http_error,
    shared_http, transport_error,
};
use crate::traits::LlmError;

#[derive(Debug, Clone)]
pub struct OpenAiConfig {
    /// Provider name for identity/logging (`local`, `openai`, `groq`, ...).
    pub provider: String,
    /// Base URL including `/v1`, e.g. `http://127.0.0.1:11434/v1`.
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub is_local: bool,
    pub timeout: Duration,
    /// `reasoning_effort` for reasoning models that accept it (Groq gpt-oss, OpenAI o-series).
    pub reasoning_effort: Option<String>,
}

impl OpenAiConfig {
    pub fn local(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: "local".into(),
            base_url: base_url.into(),
            api_key: None,
            model: model.into(),
            is_local: true,
            timeout: Duration::from_secs(120),
            reasoning_effort: None,
        }
    }
}

pub struct OpenAiCompatible {
    config: OpenAiConfig,
    http: reqwest::Client,
    json_schema_supported: AtomicBool,
    native_tools: AtomicBool,
}

impl OpenAiCompatible {
    pub fn new(mut config: OpenAiConfig) -> Self {
        config.base_url = config.base_url.trim().trim_end_matches('/').to_owned();
        Self {
            config,
            http: shared_http(),
            json_schema_supported: AtomicBool::new(true),
            native_tools: AtomicBool::new(true),
        }
    }

    pub fn config(&self) -> &OpenAiConfig {
        &self.config
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.config.base_url, path)
    }

    async fn post(&self, body: &Value) -> Result<Value, LlmError> {
        let provider = &self.config.provider;
        let mut req = self.http.post(self.url("chat/completions")).timeout(self.config.timeout).json(body);
        if let Some(key) = self.config.api_key.as_deref().filter(|k| !k.is_empty()) {
            req = req.bearer_auth(key);
        }
        let response = req.send().await.map_err(|e| transport_error(provider, e))?;
        if !response.status().is_success() {
            return Err(http_error(provider, response).await);
        }
        response
            .json::<Value>()
            .await
            .map_err(|_| LlmError::with_code(format!("{provider} returned a non-JSON body"), codes::BAD_RESPONSE))
    }

    fn json_body(&self, request: &JsonRequest, use_schema: bool) -> Value {
        let mut system = request.system.clone();
        if !use_schema {
            system.push_str(&request.schema_instruction());
        }
        let mut messages =
            vec![json!({"role": "system", "content": system}), json!({"role": "user", "content": request.user})];
        if let Some(repair) = &request.repair {
            messages.push(json!({"role": "assistant", "content": repair.previous_output}));
            messages.push(json!({"role": "user", "content": JsonRequest::repair_instruction(repair)}));
        }
        let response_format = if use_schema {
            json!({"type": "json_schema", "json_schema": {"name": request.schema_name, "schema": request.schema, "strict": true}})
        } else {
            json!({"type": "json_object"})
        };
        let mut body = json!({
            "model": self.config.model,
            "messages": messages,
            "temperature": request.temperature,
            "max_tokens": request.max_output_tokens,
            "response_format": response_format,
            "stream": false,
        });
        if let Some(effort) = &self.config.reasoning_effort {
            body["reasoning_effort"] = json!(effort);
        }
        body
    }

    fn message_text(body: &Value) -> Option<String> {
        let message = body.pointer("/choices/0/message")?;
        match message.get("content") {
            Some(Value::String(s)) if !s.trim().is_empty() => Some(s.clone()),
            // Some servers return content parts.
            Some(Value::Array(parts)) => {
                let text: String = parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect();
                (!text.trim().is_empty()).then_some(text)
            }
            _ => None,
        }
    }
}

fn mentions_schema_rejection(err: &LlmError) -> bool {
    if !matches!(err.status_code, Some(400 | 404 | 422 | 500 | 501)) {
        return false;
    }
    let m = err.message.to_lowercase();
    m.contains("response_format") || m.contains("json_schema") || m.contains("schema") || m.contains("structured")
}

fn mentions_tool_rejection(err: &LlmError) -> bool {
    matches!(err.status_code, Some(400 | 404 | 422 | 500 | 501)) && {
        let m = err.message.to_lowercase();
        m.contains("does not support tools") || m.contains("tool") && (m.contains("support") || m.contains("unknown"))
    }
}

#[async_trait]
impl ChatClient for OpenAiCompatible {
    fn identity(&self) -> ClientIdentity {
        ClientIdentity {
            provider: self.config.provider.clone(),
            model: self.config.model.clone(),
            endpoint: self.config.base_url.clone(),
            is_local: self.config.is_local,
        }
    }

    async fn respond_json(&self, request: &JsonRequest) -> Result<String, LlmError> {
        let want_schema = request.mode == JsonMode::Schema && self.json_schema_supported.load(Ordering::Relaxed);
        let body = self.json_body(request, want_schema);
        let result = match self.post(&body).await {
            Err(err) if want_schema && mentions_schema_rejection(&err) => {
                tracing::info!(provider = %self.config.provider, "server rejected json_schema; using json_object mode");
                // Groq's strict decoder can fail on one generation without lacking support.
                if err.error_code.as_deref() != Some("json_validate_failed") {
                    self.json_schema_supported.store(false, Ordering::Relaxed);
                }
                self.post(&self.json_body(request, false)).await?
            }
            Err(err) if want_schema && err.error_code.as_deref() == Some("json_validate_failed") => {
                self.post(&self.json_body(request, false)).await?
            }
            other => other?,
        };
        Self::message_text(&result).ok_or_else(|| {
            LlmError::with_code(
                format!("{} response contained no message text", self.config.provider),
                codes::BAD_RESPONSE,
            )
        })
    }

    async fn respond_with_tools(
        &self,
        system: &str,
        messages: &[ChatMessage],
        tools: &[ToolSpec],
    ) -> Result<ToolTurn, LlmError> {
        let mut wire = vec![json!({"role": "system", "content": system})];
        for message in messages {
            wire.push(match message {
                ChatMessage::User { content } => json!({"role": "user", "content": content}),
                ChatMessage::Assistant { content, tool_calls } => {
                    let mut m = json!({"role": "assistant", "content": content.clone().unwrap_or_default()});
                    if !tool_calls.is_empty() {
                        m["tool_calls"] = Value::Array(
                            tool_calls
                                .iter()
                                .map(|c| {
                                    json!({"id": c.id, "type": "function", "function": {"name": c.name, "arguments": c.arguments.to_string()}})
                                })
                                .collect(),
                        );
                    }
                    m
                }
                ChatMessage::Tool { call_id, name, content } => {
                    json!({"role": "tool", "tool_call_id": call_id, "name": name, "content": content})
                }
            });
        }
        let tool_defs: Vec<Value> = tools
            .iter()
            .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.parameters}}))
            .collect();
        let mut body = json!({"model": self.config.model, "messages": wire, "temperature": 0.0, "stream": false});
        if !tool_defs.is_empty() {
            body["tools"] = Value::Array(tool_defs);
        }
        let result = match self.post(&body).await {
            Err(mut err) if mentions_tool_rejection(&err) => {
                self.native_tools.store(false, Ordering::Relaxed);
                err.error_code = Some(codes::TOOLS_UNSUPPORTED.into());
                return Err(err);
            }
            other => other?,
        };
        let mut turn = ToolTurn { text: Self::message_text(&result), tool_calls: Vec::new() };
        if let Some(calls) = result.pointer("/choices/0/message/tool_calls").and_then(Value::as_array) {
            for (i, call) in calls.iter().enumerate() {
                let Some(function) = call.get("function") else { continue };
                let name = function.get("name").and_then(Value::as_str).unwrap_or_default().to_owned();
                let arguments = match function.get("arguments") {
                    Some(Value::String(s)) => serde_json::from_str(s)
                        .map_err(|_| LlmError::with_code("tool arguments were not valid JSON", codes::BAD_RESPONSE))?,
                    Some(v @ Value::Object(_)) => v.clone(),
                    _ => json!({}),
                };
                let id =
                    call.get("id").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| format!("call_{i}"));
                turn.tool_calls.push(ToolCall { id, name, arguments, signature: None });
            }
        }
        Ok(turn)
    }

    fn supports_native_tools(&self) -> bool {
        self.native_tools.load(Ordering::Relaxed)
    }
}
