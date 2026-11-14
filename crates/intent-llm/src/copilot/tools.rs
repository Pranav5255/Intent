//! Read-only copilot tools with a strict model-safe projection (port of 1.x `tools.py`).

use async_trait::async_trait;
use intent_types::{CurrentIntent, Intent, ResumePayload, SAFE_INTENT_PRIVACY_POLICY_VERSION, SearchHit};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::client::ToolSpec;

/// Aggregate statistics returned by the daemon for `get_intent_stats`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IntentStatsReport {
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub intent_count: i64,
    pub total_duration_seconds: i64,
    pub event_count: i64,
    #[serde(default)]
    pub by_date: Vec<DayIntentStats>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DayIntentStats {
    pub date: String,
    pub intent_count: i64,
    pub duration_seconds: i64,
}

/// Implemented by the daemon over the store. Errors are short, safe messages.
#[async_trait]
pub trait CopilotBackend: Send + Sync {
    async fn search_intents(
        &self,
        query: &str,
        limit: usize,
        date_from: Option<&str>,
        date_to: Option<&str>,
    ) -> Result<Vec<SearchHit>, String>;

    async fn get_intent(&self, intent_id: &str) -> Result<Option<Intent>, String>;

    /// Exact restore data for local response assembly (never sent to the model).
    async fn get_resume_payload(&self, intent_id: &str) -> Result<Option<ResumePayload>, String> {
        Ok(self.get_intent(intent_id).await?.map(|i| i.resume_payload))
    }

    async fn get_current_intent(&self) -> Result<Option<CurrentIntent>, String>;

    async fn get_intent_stats(
        &self,
        date_from: &str,
        date_to: &str,
        project: Option<&str>,
    ) -> Result<IntentStatsReport, String>;
}

pub const ALLOWED_TOOLS: &[&str] =
    &["search_intents", "get_intent", "get_resume_payload", "get_current_intent", "get_intent_stats"];

#[derive(Debug, Clone, Copy)]
pub struct ToolLimits {
    pub max_tool_calls: usize,
    pub max_results: usize,
    pub max_query_chars: usize,
}

impl Default for ToolLimits {
    fn default() -> Self {
        Self { max_tool_calls: 8, max_results: 10, max_query_chars: 200 }
    }
}

pub fn tool_specs(limits: &ToolLimits) -> Vec<ToolSpec> {
    let id_params = json!({
        "type": "object",
        "properties": {"intent_id": {"type": "string", "minLength": 1, "maxLength": 128}},
        "required": ["intent_id"],
        "additionalProperties": false
    });
    vec![
        ToolSpec {
            name: "search_intents".into(),
            description: "Search stored intent summaries.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "maxLength": limits.max_query_chars},
                    "limit": {"type": "integer", "minimum": 1, "maximum": limits.max_results},
                    "date_from": {"type": "string"},
                    "date_to": {"type": "string"}
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        },
        ToolSpec {
            name: "get_intent".into(),
            description: "Retrieve one stored intent.".into(),
            parameters: id_params.clone(),
        },
        ToolSpec {
            name: "get_resume_payload".into(),
            description: "Check whether restore context is available for one intent.".into(),
            parameters: id_params,
        },
        ToolSpec {
            name: "get_current_intent".into(),
            description: "Retrieve the current inferred intent.".into(),
            parameters: json!({"type": "object", "properties": {}, "additionalProperties": false}),
        },
        ToolSpec {
            name: "get_intent_stats".into(),
            description: "Aggregate stored intent statistics.".into(),
            parameters: json!({
                "type": "object",
                "properties": {"date_from": {"type": "string"}, "date_to": {"type": "string"}, "project": {"type": "string"}},
                "required": ["date_from", "date_to"],
                "additionalProperties": false
            }),
        },
    ]
}

/// A real `YYYY-MM-DD` calendar date.
pub fn valid_date(value: &str) -> bool {
    let b = value.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let num = |s: &str| s.chars().all(|c| c.is_ascii_digit()).then(|| s.parse::<u32>().ok()).flatten();
    let (Some(y), Some(m), Some(d)) = (num(&value[0..4]), num(&value[5..7]), num(&value[8..10])) else { return false };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&d) && y >= 1
}

fn invalid(message: impl Into<String>) -> Value {
    json!({"error": message.into(), "code": "invalid_args"})
}

fn non_negative(v: Option<&Value>) -> i64 {
    v.and_then(Value::as_i64).filter(|n| *n >= 0).unwrap_or(0)
}

fn string_arg(args: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.trim().to_owned())),
        _ => Err(format!("{key} must be a non-empty string")),
    }
}

fn date_arg(args: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match string_arg(args, key)? {
        Some(d) if valid_date(&d) => Ok(Some(d)),
        Some(_) => Err(format!("{key} must be a real YYYY-MM-DD date")),
        None => Ok(None),
    }
}

fn safe_public_text(intent: &Intent) -> (String, String) {
    if intent.privacy_policy_version == SAFE_INTENT_PRIVACY_POLICY_VERSION {
        (intent.label.clone(), intent.summary.clone())
    } else {
        ("Stored Work".into(), "A legacy stored intent is available locally.".into())
    }
}

fn source_family(source: &str) -> &'static str {
    match source {
        "vscode" => "editor",
        "firefox" | "chrome" => "browser",
        "shell" => "command",
        "linux" => "focus",
        "filesystem" => "file_change",
        _ => "other",
    }
}

const SAFE_SHELL_FAMILIES: &[&str] =
    &["terraform", "git", "pytest", "python", "npm", "pip", "docker", "make", "cargo", "go"];

/// Allowlisted, non-raw intent fields exposed to a model.
pub fn safe_intent(intent: &Intent) -> Value {
    let (label, summary) = safe_public_text(intent);
    let mut families: std::collections::BTreeMap<&str, i64> = std::collections::BTreeMap::new();
    for (source, count) in &intent.stats.sources {
        if *count > 0 {
            *families.entry(source_family(source)).or_default() += count;
        }
    }
    let editor: Vec<Value> = intent
        .insights
        .editor
        .iter()
        .map(|item| {
            let mut e =
                json!({"typed_chars": non_negative(item.get("typed_chars")), "saves": non_negative(item.get("saves"))});
            if let Some(f) = item.get("file").and_then(Value::as_str).map(str::trim).filter(|f| !f.is_empty()) {
                e["file"] = json!(crate::util::truncate_chars(f, 120));
            }
            e
        })
        .collect();
    let browser: Vec<Value> = intent
        .insights
        .browser
        .iter()
        .map(|item| {
            let mut e = json!({"visits": non_negative(item.get("visits"))});
            if let Some(d) = item.get("domain").and_then(Value::as_str).map(str::trim).filter(|d| !d.is_empty()) {
                e["domain"] = json!(crate::util::truncate_chars(d, 120));
            }
            e
        })
        .collect();
    let shell: Vec<Value> = intent
        .insights
        .shell
        .iter()
        .map(|item| {
            let family = item
                .get("command_family")
                .and_then(Value::as_str)
                .filter(|f| SAFE_SHELL_FAMILIES.contains(f))
                .unwrap_or("unknown");
            json!({
                "command_family": family,
                "exit_code": item.get("exit_code").and_then(Value::as_i64),
                "count": non_negative(item.get("count")),
            })
        })
        .collect();
    let semantic = intent.semantic.as_ref().map(|s| {
        let roles: Map<String, Value> = s
            .event_roles
            .iter()
            .filter(|(r, _)| matches!(r.as_str(), "task" | "supporting_context" | "background" | "unrelated"))
            .map(|(r, c)| (r.clone(), json!((*c).max(0))))
            .collect();
        json!({
            "refined": s.refined,
            "confidence": s.confidence,
            "topic": s.topic.as_deref().map(|t| crate::util::truncate_chars(t, 80)),
            "event_roles": roles,
        })
    });
    json!({
        "id": intent.id,
        "parent_id": intent.parent_id,
        "date": intent.date,
        "label": label,
        "summary": summary,
        "confidence": intent.confidence,
        "start_ts": intent.start_ts,
        "end_ts": intent.end_ts,
        "depth": intent.depth,
        "stats": {
            "event_count": intent.stats.event_count,
            "duration_seconds": intent.stats.duration_seconds,
            "activity_families": families,
        },
        "insights": {"editor": editor, "browser": browser, "shell": shell},
        "todo_count": intent.todos.len(),
        "semantic": semantic,
        "children": intent.children.iter().map(safe_intent).collect::<Vec<_>>(),
    })
}

fn safe_resume_metadata(intent_id: &str, payload: &ResumePayload) -> Value {
    json!({
        "intent_id": intent_id,
        "resume_payload_available": true,
        "resume_context": {
            "file_count": payload.files.len(),
            "url_count": payload.urls.len(),
            "has_shell_context": payload.shell.cwd.is_some() || payload.shell.last_cmd.is_some(),
        }
    })
}

fn safe_stats(stats: &IntentStatsReport) -> Value {
    json!({
        "date_from": stats.date_from,
        "date_to": stats.date_to,
        "intent_count": stats.intent_count.max(0),
        "total_duration_seconds": stats.total_duration_seconds.max(0),
        "event_count": stats.event_count.max(0),
        "by_date": stats.by_date.iter().map(|d| json!({
            "date": d.date, "intent_count": d.intent_count.max(0), "duration_seconds": d.duration_seconds.max(0)
        })).collect::<Vec<_>>(),
    })
}

/// Per-request tool executor (the call budget is per request, not shared state).
pub struct ToolSession<'a> {
    backend: &'a dyn CopilotBackend,
    limits: ToolLimits,
    calls: usize,
}

impl<'a> ToolSession<'a> {
    pub fn new(backend: &'a dyn CopilotBackend, limits: ToolLimits) -> Self {
        Self { backend, limits, calls: 0 }
    }

    pub fn calls(&self) -> usize {
        self.calls
    }

    pub async fn execute(&mut self, name: &str, arguments: &Value) -> Value {
        self.calls += 1;
        if self.calls > self.limits.max_tool_calls {
            return json!({"error": "tool call limit exceeded", "code": "tool_call_cap"});
        }
        if !ALLOWED_TOOLS.contains(&name) {
            return invalid("unknown tool name");
        }
        let empty = Map::new();
        let args = match arguments {
            Value::Object(m) => m,
            Value::Null => &empty,
            _ => return invalid("arguments must be an object"),
        };
        match self.run(name, args).await {
            Ok(v) => v,
            Err(ToolFailure::Invalid(m)) => invalid(m),
            Err(ToolFailure::Backend) => json!({"error": "tool execution failed", "code": "tool_error"}),
        }
    }

    async fn run(&self, name: &str, args: &Map<String, Value>) -> Result<Value, ToolFailure> {
        let b = self.backend;
        match name {
            "search_intents" => {
                let query = string_arg(args, "query")?
                    .ok_or_else(|| ToolFailure::Invalid("query must be a non-empty string".into()))?;
                if query.chars().count() > self.limits.max_query_chars {
                    return Err(ToolFailure::Invalid("query exceeds maximum length".into()));
                }
                let limit = match args.get("limit") {
                    None | Some(Value::Null) => self.limits.max_results as i64,
                    Some(v) => v
                        .as_i64()
                        .or_else(|| v.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))
                        .ok_or_else(|| ToolFailure::Invalid("limit must be an integer".into()))?,
                };
                if limit < 1 || limit as usize > self.limits.max_results {
                    return Err(ToolFailure::Invalid(format!(
                        "limit must be between 1 and {}",
                        self.limits.max_results
                    )));
                }
                let date_from = date_arg(args, "date_from")?;
                let date_to = date_arg(args, "date_to")?;
                if let (Some(f), Some(t)) = (&date_from, &date_to)
                    && f > t
                {
                    return Err(ToolFailure::Invalid("date_from must not be later than date_to".into()));
                }
                let hits = b
                    .search_intents(&query, limit as usize, date_from.as_deref(), date_to.as_deref())
                    .await
                    .map_err(|_| ToolFailure::Backend)?;
                let mut records = Vec::new();
                for hit in hits {
                    let mut record = json!({"id": hit.id, "date": hit.date});
                    if let Ok(Some(intent)) = b.get_intent(&hit.id).await {
                        let (label, summary) = safe_public_text(&intent);
                        record["label"] = json!(label);
                        record["summary"] = json!(summary);
                    }
                    records.push(record);
                }
                Ok(json!({"results": records}))
            }
            "get_intent" => {
                let id = intent_id(args)?;
                Ok(match b.get_intent(&id).await.map_err(|_| ToolFailure::Backend)? {
                    Some(intent) => json!({"intent": safe_intent(&intent)}),
                    None => json!({"error": "not_found"}),
                })
            }
            "get_resume_payload" => {
                let id = intent_id(args)?;
                Ok(match b.get_resume_payload(&id).await.map_err(|_| ToolFailure::Backend)? {
                    Some(payload) => safe_resume_metadata(&id, &payload),
                    None => json!({"error": "not_found"}),
                })
            }
            "get_current_intent" => {
                let current = b.get_current_intent().await.map_err(|_| ToolFailure::Backend)?;
                Ok(json!({"current_intent": current.map(|c| json!({
                    "label": c.label, "summary": c.summary, "confidence": c.confidence, "since_ts": c.since_ts
                }))}))
            }
            _ => {
                let from = date_arg(args, "date_from")?
                    .ok_or_else(|| ToolFailure::Invalid("date_from must be a non-empty string".into()))?;
                let to = date_arg(args, "date_to")?
                    .ok_or_else(|| ToolFailure::Invalid("date_to must be a non-empty string".into()))?;
                let project = string_arg(args, "project")?;
                let stats =
                    b.get_intent_stats(&from, &to, project.as_deref()).await.map_err(|_| ToolFailure::Backend)?;
                Ok(json!({"stats": safe_stats(&stats)}))
            }
        }
    }
}

enum ToolFailure {
    Invalid(String),
    Backend,
}

impl From<String> for ToolFailure {
    fn from(s: String) -> Self {
        ToolFailure::Invalid(s)
    }
}

fn intent_id(args: &Map<String, Value>) -> Result<String, ToolFailure> {
    let id = string_arg(args, "intent_id")?
        .ok_or_else(|| ToolFailure::Invalid("intent_id must be a non-empty string".into()))?;
    if id.chars().count() > 128 {
        return Err(ToolFailure::Invalid("intent_id must be at most 128 characters".into()));
    }
    Ok(id)
}

const BLOCKED_KEYS: &[&str] = &[
    "raw",
    "evidence",
    "payload",
    "content",
    "document",
    "changes",
    "text",
    "resume_payload",
    "files",
    "file",
    "path",
    "urls",
    "url",
    "shell",
    "cwd",
    "command",
    "title",
    "domain",
    "tags",
    "todos",
    "prefix",
    "workspace_root",
    "provider_identity",
    "highlight_snippet",
];

/// Second boundary before a tool result joins an LLM request.
pub fn model_tool_result(name: &str, result: &Value) -> Value {
    if !result.is_object() {
        return json!({"error": "invalid_tool_result"});
    }
    if name == "get_resume_payload" {
        let ctx = result.get("resume_context").cloned().unwrap_or_default();
        return json!({
            "intent_id": result.get("intent_id").and_then(Value::as_str),
            "resume_payload_available": result.get("resume_payload_available").and_then(Value::as_bool).unwrap_or(false),
            "resume_context": {
                "file_count": non_negative(ctx.get("file_count")),
                "url_count": non_negative(ctx.get("url_count")),
                "has_shell_context": ctx.get("has_shell_context").and_then(Value::as_bool).unwrap_or(false),
            }
        });
    }
    strip_private_fields(result)
}

pub fn strip_private_fields(value: &Value) -> Value {
    match value {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(k, _)| !BLOCKED_KEYS.contains(&k.to_lowercase().as_str()))
                .map(|(k, v)| (k.clone(), strip_private_fields(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(strip_private_fields).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert!(valid_date("2024-02-29"));
        assert!(!valid_date("2023-02-29"));
        assert!(!valid_date("2024-13-01"));
        assert!(!valid_date("2024-1-01"));
        assert!(!valid_date("yesterday"));
    }

    #[test]
    fn strips_private_fields_recursively() {
        let v = json!({"intent": {"label": "x", "insights": {"editor": [{"file": "a.rs", "saves": 1}]}, "raw": {}, "Title": "t"}});
        let out = strip_private_fields(&v);
        assert_eq!(out, json!({"intent": {"label": "x", "insights": {"editor": [{"saves": 1}]}}}));
    }

    #[test]
    fn resume_result_is_reduced_to_counts() {
        let v = json!({"intent_id": "i1", "resume_payload_available": true, "resume_context": {"file_count": 2, "url_count": -1, "has_shell_context": true}, "files": ["/x"]});
        let out = model_tool_result("get_resume_payload", &v);
        assert_eq!(out["resume_context"]["file_count"], 2);
        assert_eq!(out["resume_context"]["url_count"], 0);
        assert!(out.get("files").is_none());
    }
}
