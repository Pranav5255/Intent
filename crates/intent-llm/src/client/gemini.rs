//! Gemini over REST: API keys (generativelanguage.googleapis.com) and Vertex AI service
//! accounts (OAuth2 JWT bearer). No SDK, no global environment mutation.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::Mutex;

use super::{
    ChatClient, ChatMessage, ClientIdentity, JsonMode, JsonRequest, ToolCall, ToolSpec, ToolTurn, codes, http_error,
    shared_http, transport_error,
};
use crate::traits::LlmError;

pub const GEMINI_API_BASE: &str = "https://generativelanguage.googleapis.com";
const VERTEX_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

/// The fields of a Google service-account JSON key that Vertex auth needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceAccountKey {
    #[serde(rename = "type")]
    pub kind: String,
    pub project_id: String,
    pub private_key: String,
    pub client_email: String,
    #[serde(default = "default_token_uri")]
    pub token_uri: String,
}

fn default_token_uri() -> String {
    "https://oauth2.googleapis.com/token".into()
}

impl ServiceAccountKey {
    /// Validate a service-account JSON document (same rules as 1.x).
    pub fn parse(raw: &[u8]) -> Result<Self, String> {
        let value: Value =
            serde_json::from_slice(raw).map_err(|_| "Gemini credentials must be valid JSON".to_owned())?;
        let obj = value.as_object().ok_or("Gemini credentials must be a JSON object")?;
        for key in ["type", "project_id", "private_key", "client_email"] {
            if obj.get(key).and_then(Value::as_str).is_none_or(|v| v.trim().is_empty()) {
                return Err(format!("Gemini credentials missing required field: {key}"));
            }
        }
        if obj["type"] != "service_account" {
            return Err("Gemini credentials must be a service account JSON file".into());
        }
        serde_json::from_value(value).map_err(|e| e.to_string())
    }
}

#[derive(Debug, Clone)]
pub enum GeminiAuth {
    ApiKey(String),
    /// Vertex AI with a service account; `project` defaults to the key's project.
    ServiceAccount {
        key: ServiceAccountKey,
        project: Option<String>,
        location: String,
    },
}

#[derive(Debug, Clone)]
pub struct GeminiConfig {
    pub auth: GeminiAuth,
    pub model: String,
    pub timeout: Duration,
    /// Override the API host (tests, proxies). For Vertex this replaces `https://{location}-aiplatform.googleapis.com`.
    pub base_url: Option<String>,
}

pub struct GeminiClient {
    config: GeminiConfig,
    http: reqwest::Client,
    token: Mutex<Option<(String, u64)>>,
}

impl GeminiClient {
    pub fn new(config: GeminiConfig) -> Self {
        Self { config, http: shared_http(), token: Mutex::new(None) }
    }

    fn endpoint(&self) -> String {
        match &self.config.auth {
            GeminiAuth::ApiKey(_) => {
                let base = self.config.base_url.as_deref().unwrap_or(GEMINI_API_BASE).trim_end_matches('/');
                format!("{base}/v1beta/models/{}:generateContent", self.config.model)
            }
            GeminiAuth::ServiceAccount { key, project, location } => {
                let project = project.as_deref().unwrap_or(&key.project_id);
                let host = match &self.config.base_url {
                    Some(b) => b.trim_end_matches('/').to_owned(),
                    None if location == "global" => "https://aiplatform.googleapis.com".into(),
                    None => format!("https://{location}-aiplatform.googleapis.com"),
                };
                format!(
                    "{host}/v1/projects/{project}/locations/{location}/publishers/google/models/{}:generateContent",
                    self.config.model
                )
            }
        }
    }

    async fn access_token(&self, key: &ServiceAccountKey) -> Result<String, LlmError> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default();
        let mut guard = self.token.lock().await;
        if let Some((token, expires)) = guard.as_ref()
            && *expires > now + 60
        {
            return Ok(token.clone());
        }
        #[derive(Serialize)]
        struct Claims<'a> {
            iss: &'a str,
            scope: &'a str,
            aud: &'a str,
            iat: u64,
            exp: u64,
        }
        let claims =
            Claims { iss: &key.client_email, scope: VERTEX_SCOPE, aud: &key.token_uri, iat: now, exp: now + 3600 };
        let signing_key = jsonwebtoken::EncodingKey::from_rsa_pem(key.private_key.as_bytes())
            .map_err(|_| LlmError::with_code("Gemini service-account private key is invalid", codes::CONFIG))?;
        let assertion =
            jsonwebtoken::encode(&jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256), &claims, &signing_key)
                .map_err(|_| LlmError::with_code("could not sign the Vertex AI token request", codes::CONFIG))?;
        let response = self
            .http
            .post(&key.token_uri)
            .timeout(Duration::from_secs(15))
            .form(&[("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"), ("assertion", assertion.as_str())])
            .send()
            .await
            .map_err(|e| transport_error("Google OAuth", e))?;
        if !response.status().is_success() {
            return Err(http_error("Google OAuth", response).await);
        }
        let body: Value = response
            .json()
            .await
            .map_err(|_| LlmError::with_code("Google OAuth returned a non-JSON body", codes::BAD_RESPONSE))?;
        let token = body
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or_else(|| LlmError::with_code("Google OAuth returned no access token", codes::BAD_RESPONSE))?
            .to_owned();
        let ttl = body.get("expires_in").and_then(Value::as_u64).unwrap_or(3600);
        *guard = Some((token.clone(), now + ttl));
        Ok(token)
    }

    async fn generate(&self, body: &Value) -> Result<Value, LlmError> {
        let mut req = self.http.post(self.endpoint()).timeout(self.config.timeout).json(body);
        req = match &self.config.auth {
            GeminiAuth::ApiKey(key) => req.header("x-goog-api-key", key),
            GeminiAuth::ServiceAccount { key, .. } => req.bearer_auth(self.access_token(key).await?),
        };
        let response = req.send().await.map_err(|e| transport_error("Gemini", e))?;
        if !response.status().is_success() {
            return Err(http_error("Gemini", response).await);
        }
        response.json().await.map_err(|_| LlmError::with_code("Gemini returned a non-JSON body", codes::BAD_RESPONSE))
    }

    fn parts(body: &Value) -> Vec<Value> {
        body.pointer("/candidates/0/content/parts").and_then(Value::as_array).cloned().unwrap_or_default()
    }

    fn text_of(parts: &[Value]) -> Option<String> {
        let text: String = parts
            .iter()
            .filter(|p| p.get("thought").and_then(Value::as_bool) != Some(true))
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect();
        (!text.trim().is_empty()).then_some(text)
    }
}

/// Convert JSON Schema to Gemini's OpenAPI subset: uppercase types, whitelisted keywords.
pub(crate) fn gemini_schema(value: &Value) -> Value {
    const KEEP: &[&str] =
        &["type", "format", "description", "nullable", "enum", "items", "minItems", "maxItems", "minimum", "maximum"];
    let Some(map) = value.as_object() else { return value.clone() };
    let mut out = Map::new();
    for (k, v) in map {
        match k.as_str() {
            "properties" => {
                let props = v
                    .as_object()
                    .map(|p| p.iter().map(|(name, s)| (name.clone(), gemini_schema(s))).collect())
                    .unwrap_or_default();
                out.insert(k.clone(), Value::Object(props));
                // Keep the model's output in the declared order: `required` is an ordered
                // array, while object keys are sorted by serde_json.
                let order = match map.get("required").and_then(Value::as_array) {
                    Some(req) if !req.is_empty() => req.clone(),
                    _ => v.as_object().map(|p| p.keys().map(|n| json!(n)).collect()).unwrap_or_default(),
                };
                out.insert("propertyOrdering".into(), Value::Array(order));
            }
            "required" => {
                out.insert(k.clone(), v.clone());
            }
            "type" => {
                let t = v.as_str().map(|s| s.to_uppercase()).unwrap_or_else(|| "STRING".into());
                out.insert(k.clone(), json!(t));
            }
            "items" => {
                out.insert(k.clone(), gemini_schema(v));
            }
            key if KEEP.contains(&key) => {
                out.insert(k.clone(), v.clone());
            }
            _ => {}
        }
    }
    Value::Object(out)
}

#[async_trait]
impl ChatClient for GeminiClient {
    fn identity(&self) -> ClientIdentity {
        let endpoint = match &self.config.auth {
            GeminiAuth::ApiKey(_) => "generativelanguage".to_owned(),
            GeminiAuth::ServiceAccount { key, project, location } => {
                format!("vertex:{}:{location}", project.as_deref().unwrap_or(&key.project_id))
            }
        };
        ClientIdentity { provider: "gemini".into(), model: self.config.model.clone(), endpoint, is_local: false }
    }

    async fn respond_json(&self, request: &JsonRequest) -> Result<String, LlmError> {
        let mut system = request.system.clone();
        let mut generation = json!({
            "temperature": request.temperature,
            "maxOutputTokens": request.max_output_tokens,
            "responseMimeType": "application/json",
        });
        match request.mode {
            JsonMode::Schema => generation["responseSchema"] = gemini_schema(&request.schema),
            JsonMode::Object => system.push_str(&request.schema_instruction()),
        }
        let mut contents = vec![json!({"role": "user", "parts": [{"text": request.user}]})];
        if let Some(repair) = &request.repair {
            contents.push(json!({"role": "model", "parts": [{"text": repair.previous_output}]}));
            contents.push(json!({"role": "user", "parts": [{"text": JsonRequest::repair_instruction(repair)}]}));
        }
        let body = json!({
            "systemInstruction": {"parts": [{"text": system}]},
            "contents": contents,
            "generationConfig": generation,
        });
        let result = self.generate(&body).await?;
        Self::text_of(&Self::parts(&result))
            .ok_or_else(|| LlmError::with_code("Gemini response contained no text", codes::BAD_RESPONSE))
    }

    async fn respond_with_tools(
        &self,
        system: &str,
        messages: &[ChatMessage],
        tools: &[ToolSpec],
    ) -> Result<ToolTurn, LlmError> {
        let mut contents: Vec<Value> = Vec::new();
        let mut pending: Vec<Value> = Vec::new();
        let flush = |contents: &mut Vec<Value>, pending: &mut Vec<Value>| {
            if !pending.is_empty() {
                contents.push(json!({"role": "user", "parts": std::mem::take(pending)}));
            }
        };
        for message in messages {
            match message {
                ChatMessage::Tool { name, content, .. } => {
                    let result: Value = serde_json::from_str(content).unwrap_or_else(|_| json!(content));
                    pending.push(json!({"functionResponse": {"name": name, "response": {"result": result}}}));
                }
                ChatMessage::User { content } => {
                    flush(&mut contents, &mut pending);
                    contents.push(json!({"role": "user", "parts": [{"text": content}]}));
                }
                ChatMessage::Assistant { content, tool_calls } => {
                    flush(&mut contents, &mut pending);
                    let mut parts = Vec::new();
                    if let Some(text) = content.as_deref().filter(|t| !t.is_empty()) {
                        parts.push(json!({"text": text}));
                    }
                    for call in tool_calls {
                        let mut part = json!({"functionCall": {"name": call.name, "args": call.arguments}});
                        if let Some(sig) = &call.signature {
                            part["thoughtSignature"] = json!(sig);
                        }
                        parts.push(part);
                    }
                    if !parts.is_empty() {
                        contents.push(json!({"role": "model", "parts": parts}));
                    }
                }
            }
        }
        flush(&mut contents, &mut pending);
        let mut body = json!({
            "systemInstruction": {"parts": [{"text": system}]},
            "contents": contents,
            "generationConfig": {"temperature": 0.0},
        });
        if !tools.is_empty() {
            let decls: Vec<Value> = tools
                .iter()
                .map(|t| json!({"name": t.name, "description": t.description, "parameters": gemini_schema(&t.parameters)}))
                .collect();
            body["tools"] = json!([{"functionDeclarations": decls}]);
        }
        let result = self.generate(&body).await?;
        let parts = Self::parts(&result);
        let mut turn = ToolTurn { text: Self::text_of(&parts), tool_calls: Vec::new() };
        for (i, part) in parts.iter().enumerate() {
            if let Some(call) = part.get("functionCall") {
                turn.tool_calls.push(ToolCall {
                    id: format!("gemini-{i}"),
                    name: call.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
                    arguments: call.get("args").cloned().unwrap_or_else(|| json!({})),
                    signature: part.get("thoughtSignature").and_then(Value::as_str).map(str::to_owned),
                });
            }
        }
        Ok(turn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_conversion() {
        let schema = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {"role": {"type": "string", "enum": ["task"]}, "n": {"type": "number", "minimum": 0}},
            "required": ["role", "n"]
        });
        let out = gemini_schema(&schema);
        assert_eq!(out["type"], "OBJECT");
        assert!(out.get("additionalProperties").is_none());
        assert_eq!(out["properties"]["role"]["enum"], json!(["task"]));
        assert_eq!(out["propertyOrdering"], json!(["role", "n"]));
    }

    #[test]
    fn service_account_validation() {
        assert!(ServiceAccountKey::parse(b"{}").is_err());
        assert!(ServiceAccountKey::parse(b"not json").is_err());
        let ok = br#"{"type":"service_account","project_id":"p","private_key":"k","client_email":"e@x"}"#;
        let key = ServiceAccountKey::parse(ok).unwrap();
        assert_eq!(key.token_uri, "https://oauth2.googleapis.com/token");
        let wrong = br#"{"type":"user","project_id":"p","private_key":"k","client_email":"e"}"#;
        assert!(ServiceAccountKey::parse(wrong).is_err());
    }
}
