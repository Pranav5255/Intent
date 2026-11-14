//! Amazon Bedrock Converse API over REST.
//!
//! Bedrock API keys (`AWS_BEARER_TOKEN_BEDROCK`-style bearer tokens) work in the default
//! build. The `bedrock` feature adds SigV4 signing with the standard AWS credential chain
//! (profiles, SSO, IAM roles) via `aws-config` + `aws-sigv4`.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{
    ChatClient, ChatMessage, ClientIdentity, JsonMode, JsonRequest, ToolCall, ToolSpec, ToolTurn, codes, http_error,
    shared_http, strip_schema_keys, transport_error,
};
use crate::traits::LlmError;

/// Bedrock structured outputs accept a subset of JSON Schema.
const UNSUPPORTED_SCHEMA_KEYS: &[&str] = &[
    "exclusiveMaximum",
    "exclusiveMinimum",
    "maxItems",
    "maxLength",
    "maxProperties",
    "maximum",
    "minLength",
    "minProperties",
    "minimum",
    "multipleOf",
    "pattern",
    "uniqueItems",
];

#[derive(Debug, Clone)]
pub enum BedrockAuth {
    /// Bedrock API key sent as a bearer token.
    ApiKey(String),
    /// Standard AWS credential chain with an optional named profile (requires the `bedrock` feature).
    AwsChain { profile: Option<String> },
}

#[derive(Debug, Clone)]
pub struct BedrockConfig {
    pub region: String,
    pub auth: BedrockAuth,
    pub model: String,
    pub timeout: Duration,
    pub max_tokens: u32,
    /// Use Converse `outputConfig` json_schema. `None` = auto (off for Nova Pro, which rejects it).
    pub structured_output: Option<bool>,
    /// Override the endpoint (tests, VPC endpoints).
    pub base_url: Option<String>,
}

pub struct BedrockClient {
    config: BedrockConfig,
    http: reqwest::Client,
    #[cfg(feature = "bedrock")]
    aws: tokio::sync::OnceCell<aws_config::SdkConfig>,
}

impl BedrockClient {
    pub fn new(config: BedrockConfig) -> Self {
        Self {
            config,
            http: shared_http(),
            #[cfg(feature = "bedrock")]
            aws: tokio::sync::OnceCell::new(),
        }
    }

    fn structured(&self) -> bool {
        self.config.structured_output.unwrap_or_else(|| !self.config.model.to_lowercase().contains("nova-pro"))
    }

    fn url(&self) -> String {
        let base = self
            .config
            .base_url
            .clone()
            .unwrap_or_else(|| format!("https://bedrock-runtime.{}.amazonaws.com", self.config.region));
        let model: String = self
            .config
            .model
            .chars()
            .map(|c| match c {
                ':' => "%3A".to_owned(),
                '/' => "%2F".to_owned(),
                c => c.to_string(),
            })
            .collect();
        format!("{}/model/{model}/converse", base.trim_end_matches('/'))
    }

    async fn converse(&self, body: &Value) -> Result<Value, LlmError> {
        let url = self.url();
        let bytes = serde_json::to_vec(body).map_err(|e| LlmError::new(e.to_string()))?;
        let mut req = self
            .http
            .post(&url)
            .timeout(self.config.timeout)
            .header("content-type", "application/json")
            .header("accept", "application/json");
        match &self.config.auth {
            BedrockAuth::ApiKey(key) => req = req.bearer_auth(key),
            BedrockAuth::AwsChain { profile } => {
                for (name, value) in self.sigv4_headers(&url, &bytes, profile.as_deref()).await? {
                    req = req.header(name, value);
                }
            }
        }
        let response = req.body(bytes).send().await.map_err(|e| transport_error("Bedrock", e))?;
        if !response.status().is_success() {
            return Err(http_error("Bedrock", response).await);
        }
        response.json().await.map_err(|_| LlmError::with_code("Bedrock returned a non-JSON body", codes::BAD_RESPONSE))
    }

    #[cfg(not(feature = "bedrock"))]
    async fn sigv4_headers(&self, _: &str, _: &[u8], _: Option<&str>) -> Result<Vec<(String, String)>, LlmError> {
        Err(LlmError::with_code(
            "Bedrock profile/IAM credentials need the `bedrock` build feature; use a Bedrock API key instead",
            codes::CONFIG,
        ))
    }

    #[cfg(feature = "bedrock")]
    async fn sigv4_headers(
        &self,
        url: &str,
        body: &[u8],
        profile: Option<&str>,
    ) -> Result<Vec<(String, String)>, LlmError> {
        use aws_credential_types::provider::ProvideCredentials;
        use aws_sigv4::http_request::{SignableBody, SignableRequest, SigningSettings, sign};

        let region = self.config.region.clone();
        let conf = self
            .aws
            .get_or_init(|| async {
                let mut loader =
                    aws_config::defaults(aws_config::BehaviorVersion::latest()).region(aws_config::Region::new(region));
                if let Some(p) = profile {
                    loader = loader.profile_name(p);
                }
                loader.load().await
            })
            .await;
        let provider = conf
            .credentials_provider()
            .ok_or_else(|| LlmError::with_code("no AWS credentials provider is configured", codes::CONFIG))?;
        let creds = provider
            .provide_credentials()
            .await
            .map_err(|_| LlmError::with_code("AWS credentials could not be resolved", codes::CONFIG))?;
        let identity = creds.into();
        let params = aws_sigv4::sign::v4::SigningParams::builder()
            .identity(&identity)
            .region(&self.config.region)
            .name("bedrock")
            .time(std::time::SystemTime::now())
            .settings(SigningSettings::default())
            .build()
            .map_err(|_| LlmError::with_code("could not build SigV4 parameters", codes::CONFIG))?
            .into();
        let host = reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_default();
        let headers = [("content-type", "application/json"), ("host", host.as_str())];
        let signable = SignableRequest::new("POST", url, headers.into_iter(), SignableBody::Bytes(body))
            .map_err(|_| LlmError::with_code("could not prepare SigV4 request", codes::CONFIG))?;
        let (instructions, _) = sign(signable, &params)
            .map_err(|_| LlmError::with_code("SigV4 signing failed", codes::CONFIG))?
            .into_parts();
        let (signed, _) = instructions.into_parts();
        Ok(signed.into_iter().map(|h| (h.name().to_owned(), h.value().to_owned())).collect())
    }

    fn message(body: &Value) -> Value {
        body.pointer("/output/message").cloned().unwrap_or(Value::Null)
    }

    fn text_of(message: &Value) -> Option<String> {
        let text: String = message
            .get("content")
            .and_then(Value::as_array)
            .map(|c| c.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect())
            .unwrap_or_default();
        (!text.trim().is_empty()).then_some(text)
    }

    fn base(&self, system: &str, messages: Vec<Value>, max_tokens: u32, temperature: f32) -> Value {
        json!({
            "system": [{"text": system}],
            "messages": messages,
            "inferenceConfig": {"maxTokens": max_tokens, "temperature": temperature},
        })
    }
}

fn structured_rejected(err: &LlmError) -> bool {
    let m = err.message.to_lowercase();
    err.status_code == Some(400)
        && (m.contains("outputconfig")
            || m.contains("output config")
            || m.contains("structured output")
            || m.contains("json_schema"))
}

#[async_trait]
impl ChatClient for BedrockClient {
    fn identity(&self) -> ClientIdentity {
        ClientIdentity {
            provider: "bedrock".into(),
            model: self.config.model.clone(),
            endpoint: self.config.region.clone(),
            is_local: false,
        }
    }

    async fn respond_json(&self, request: &JsonRequest) -> Result<String, LlmError> {
        let mut messages = vec![json!({"role": "user", "content": [{"text": request.user}]})];
        if let Some(repair) = &request.repair {
            messages.push(json!({"role": "assistant", "content": [{"text": repair.previous_output}]}));
            messages.push(json!({"role": "user", "content": [{"text": JsonRequest::repair_instruction(repair)}]}));
        }
        let schema = strip_schema_keys(&request.schema, UNSUPPORTED_SCHEMA_KEYS);
        let prompt_system = format!("{}{}", request.system, request.schema_instruction());
        let fallback = self.base(&prompt_system, messages.clone(), request.max_output_tokens, request.temperature);
        let result = if request.mode == JsonMode::Schema && self.structured() {
            let mut body = self.base(&request.system, messages, request.max_output_tokens, request.temperature);
            let name: String = request
                .schema_name
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
                .take(64)
                .collect();
            body["outputConfig"] = json!({"textFormat": {"type": "json_schema", "structure": {"jsonSchema": {
                "name": name, "description": "Intent structured response", "schema": schema.to_string()
            }}}});
            match self.converse(&body).await {
                Err(err) if structured_rejected(&err) => self.converse(&fallback).await?,
                other => other?,
            }
        } else {
            self.converse(&fallback).await?
        };
        Self::text_of(&Self::message(&result))
            .ok_or_else(|| LlmError::with_code("Bedrock response contained no text", codes::BAD_RESPONSE))
    }

    async fn respond_with_tools(
        &self,
        system: &str,
        messages: &[ChatMessage],
        tools: &[ToolSpec],
    ) -> Result<ToolTurn, LlmError> {
        let mut wire: Vec<Value> = Vec::new();
        let mut pending: Vec<Value> = Vec::new();
        for message in messages {
            match message {
                ChatMessage::Tool { call_id, content, .. } => {
                    let result: Value = serde_json::from_str(content).unwrap_or_else(|_| json!({"text": content}));
                    let block = if result.is_object() { json!({"json": result}) } else { json!({"text": content}) };
                    pending.push(json!({"toolResult": {"toolUseId": call_id, "content": [block]}}));
                }
                ChatMessage::User { content } => {
                    if !pending.is_empty() {
                        wire.push(json!({"role": "user", "content": std::mem::take(&mut pending)}));
                    }
                    wire.push(json!({"role": "user", "content": [{"text": content}]}));
                }
                ChatMessage::Assistant { content, tool_calls } => {
                    if !pending.is_empty() {
                        wire.push(json!({"role": "user", "content": std::mem::take(&mut pending)}));
                    }
                    let mut blocks = Vec::new();
                    if let Some(text) = content.as_deref().filter(|t| !t.is_empty()) {
                        blocks.push(json!({"text": text}));
                    }
                    for call in tool_calls {
                        blocks.push(
                            json!({"toolUse": {"toolUseId": call.id, "name": call.name, "input": call.arguments}}),
                        );
                    }
                    if !blocks.is_empty() {
                        wire.push(json!({"role": "assistant", "content": blocks}));
                    }
                }
            }
        }
        if !pending.is_empty() {
            wire.push(json!({"role": "user", "content": pending}));
        }
        let mut body = self.base(system, wire, self.config.max_tokens, 0.0);
        if !tools.is_empty() {
            let specs: Vec<Value> = tools
                .iter()
                .map(|t| {
                    json!({"toolSpec": {"name": t.name, "description": t.description, "inputSchema": {"json": t.parameters}}})
                })
                .collect();
            body["toolConfig"] = json!({"tools": specs});
        }
        let result = self.converse(&body).await?;
        let message = Self::message(&result);
        let mut turn = ToolTurn { text: Self::text_of(&message), tool_calls: Vec::new() };
        for block in message.get("content").and_then(Value::as_array).into_iter().flatten() {
            if let Some(tool) = block.get("toolUse") {
                turn.tool_calls.push(ToolCall {
                    id: tool.get("toolUseId").and_then(Value::as_str).unwrap_or_default().to_owned(),
                    name: tool.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
                    arguments: tool.get("input").cloned().unwrap_or_else(|| json!({})),
                    signature: None,
                });
            }
        }
        Ok(turn)
    }
}
