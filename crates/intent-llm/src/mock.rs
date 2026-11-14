//! Offline chat clients for tests and the eval harness.

use std::collections::{BTreeSet, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::client::{ChatClient, ChatMessage, ClientIdentity, JsonRequest, ToolSpec, ToolTurn};
use crate::traits::LlmError;

/// Replays queued responses in order and records every request.
pub struct ScriptedClient {
    identity: ClientIdentity,
    json: Mutex<VecDeque<Result<String, LlmError>>>,
    tools: Mutex<VecDeque<Result<ToolTurn, LlmError>>>,
    pub requests: Mutex<Vec<JsonRequest>>,
    pub tool_requests: Mutex<Vec<Vec<ChatMessage>>>,
    native_tools: bool,
}

impl ScriptedClient {
    pub fn new(is_local: bool) -> Self {
        Self {
            identity: ClientIdentity {
                provider: "mock".into(),
                model: "scripted".into(),
                endpoint: "memory".into(),
                is_local,
            },
            json: Mutex::new(VecDeque::new()),
            tools: Mutex::new(VecDeque::new()),
            requests: Mutex::new(Vec::new()),
            tool_requests: Mutex::new(Vec::new()),
            native_tools: true,
        }
    }

    pub fn without_native_tools(mut self) -> Self {
        self.native_tools = false;
        self
    }

    pub fn push_json(&self, response: Result<String, LlmError>) -> &Self {
        self.json.lock().expect("lock").push_back(response);
        self
    }

    pub fn push_tools(&self, response: Result<ToolTurn, LlmError>) -> &Self {
        self.tools.lock().expect("lock").push_back(response);
        self
    }

    pub fn json_calls(&self) -> usize {
        self.requests.lock().expect("lock").len()
    }
}

#[async_trait]
impl ChatClient for ScriptedClient {
    fn identity(&self) -> ClientIdentity {
        self.identity.clone()
    }

    async fn respond_json(&self, request: &JsonRequest) -> Result<String, LlmError> {
        self.requests.lock().expect("lock").push(request.clone());
        self.json.lock().expect("lock").pop_front().unwrap_or_else(|| Err(LlmError::new("script exhausted")))
    }

    async fn respond_with_tools(
        &self,
        _: &str,
        messages: &[ChatMessage],
        _: &[ToolSpec],
    ) -> Result<ToolTurn, LlmError> {
        self.tool_requests.lock().expect("lock").push(messages.to_vec());
        self.tools.lock().expect("lock").pop_front().unwrap_or_else(|| Err(LlmError::new("script exhausted")))
    }

    fn supports_native_tools(&self) -> bool {
        self.native_tools
    }
}

const NONWORK_DOMAINS: &[&str] = &[
    "youtube.com",
    "reddit.com",
    "netflix.com",
    "news.ycombinator.com",
    "lobste.rs",
    "twitter.com",
    "x.com",
    "mail.google.com",
    "calendar.google.com",
];

/// A deterministic stand-in for a small model on the compact semantic prompt: it links
/// packets that share projects/files/domains and makes borderline (0.62) links on
/// similarity hints alone, which exercises the embedding tie-breaker.
pub struct HeuristicClient {
    pub calls: AtomicUsize,
}

impl Default for HeuristicClient {
    fn default() -> Self {
        Self { calls: AtomicUsize::new(0) }
    }
}

fn strings(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

#[async_trait]
impl ChatClient for HeuristicClient {
    fn identity(&self) -> ClientIdentity {
        ClientIdentity { provider: "mock".into(), model: "heuristic".into(), endpoint: "memory".into(), is_local: true }
    }

    async fn respond_json(&self, request: &JsonRequest) -> Result<String, LlmError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let input: Value =
            serde_json::from_str(&request.user).map_err(|_| LlmError::new("heuristic mock expects JSON input"))?;
        let packets = input.get("packets").and_then(Value::as_array).ok_or_else(|| LlmError::new("no packets"))?;
        struct Info {
            id: String,
            work: bool,
            keys: BTreeSet<String>,
        }
        let mut infos: Vec<Info> = Vec::new();
        let mut proposals = Vec::new();
        for p in packets {
            let id = p.get("packet_id").and_then(Value::as_str).unwrap_or_default().to_owned();
            let background = p.get("deterministic_role").and_then(Value::as_str) == Some("background");
            let domains = strings(p, "domains");
            let projects = strings(p, "projects");
            let files = strings(p, "files");
            let types = strings(p, "types");
            let nonwork = !domains.is_empty()
                && projects.is_empty()
                && files.is_empty()
                && domains.iter().all(|d| NONWORK_DOMAINS.iter().any(|n| d.ends_with(n)));
            let browser_only = types.iter().all(|t| t.starts_with("browser/"));
            let role = if background {
                "background"
            } else if nonwork {
                "unrelated"
            } else if browser_only {
                "supporting_context"
            } else {
                "task"
            };
            let work = matches!(role, "task" | "supporting_context");
            let keys: BTreeSet<String> = projects
                .iter()
                .chain(files.iter())
                .cloned()
                .chain(domains.iter().filter(|_| !nonwork).cloned())
                .collect();
            let sim_prev = p.get("sim_prev").and_then(Value::as_f64).unwrap_or(0.0);
            let mut links = Vec::new();
            let mut confidence = 0.85;
            if work {
                let recent: Vec<&Info> = infos.iter().rev().filter(|i| i.work).take(4).collect();
                if let Some(hit) = recent.iter().find(|i| !i.keys.is_disjoint(&keys)) {
                    links.push(hit.id.clone());
                } else if let Some(prev) = infos.last().filter(|i| i.work)
                    && sim_prev >= 0.3
                {
                    links.push(prev.id.clone());
                    confidence = 0.62;
                }
            }
            let topic =
                projects.first().or(files.first()).or(domains.first()).cloned().unwrap_or_else(|| "unknown".into());
            proposals.push(json!({"event_id": id, "role": role, "confidence": confidence, "topic": topic, "linked_event_ids": links}));
            infos.push(Info { id, work, keys });
        }
        Ok(json!({"proposals": proposals}).to_string())
    }

    async fn respond_with_tools(&self, _: &str, _: &[ChatMessage], _: &[ToolSpec]) -> Result<ToolTurn, LlmError> {
        Ok(ToolTurn { text: Some("heuristic mock has no tools".into()), tool_calls: vec![] })
    }
}
