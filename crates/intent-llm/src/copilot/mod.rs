//! Tool-grounded Intent Copilot (port of 1.x `copilot.py`).
//!
//! Native function calling where the model supports it; otherwise a JSON "action"
//! protocol (`{"action": "search_intents", ...}` / `{"action": "answer", ...}`) through the
//! structured-output harness, which small local models follow reliably.

pub mod tools;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use intent_types::{
    CitedIntent, CopilotMode, CopilotQueryRequest, CopilotQueryResponse, EvidenceStatus, ResumeProposal,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use tools::{
    CopilotBackend, DayIntentStats, IntentStatsReport, ToolLimits, ToolSession, model_tool_result, tool_specs,
};

use crate::client::{ChatMessage, JsonMode, ToolCall, codes};
use crate::harness::redact::redact_text;
use crate::harness::{Harness, HarnessError, StructuredCall};
use crate::rewrite::rewrite_search_queries;
use crate::traits::LlmError;
use crate::util::{Lru, truncate_chars};

const SYSTEM_PROMPT: &str = "Answer only using tool results. If evidence is insufficient, say so clearly and set \
evidence_status to insufficient. Never invent files, URLs, commands, or dates. \
For resume requests, propose only an intent_id returned by a tool. The application \
attaches verified restore context locally; never list or invent restore fields.";

const QA_PROMPT: &str = " For grounded Q&A, retrieve evidence with tools before answering. Answer questions \
such as what was being fixed yesterday afternoon using only returned tool evidence. \
When available, include failed commands from insights.shell, and cite the relevant intent IDs.";

const BRIEFING_PROMPT: &str = " For briefing mode, retrieve the intent and its resume payload before answering. \
Write a concise 2-3 sentence briefing using only returned safe intent and insight \
evidence, cite the intent ID, and never invent or modify restore fields.";

const NARRATIVE_PROMPT: &str = " For narrative mode, use only the retrieved aggregate statistics and safe supporting \
tool results. Write a short human weekly narrative. Treat event counts, durations, \
intent counts, labels, projects, and dates in stats as authoritative. Never invent \
numbers, dates, files, URLs, commands, or resume context.";

const ACTION_PROTOCOL: &str = "\nYou cannot call functions directly. Each reply is ONE JSON object choosing an action:\n\
- search_intents: set query (and optionally limit, date_from, date_to)\n\
- get_intent / get_resume_payload: set intent_id\n\
- get_current_intent: no arguments\n\
- get_intent_stats: set date_from and date_to (YYYY-MM-DD), optionally project\n\
- answer: set answer to your final reply once you have evidence (or to say evidence is insufficient)\n\
Use \"\" or 0 for unused fields. Example: {\"action\":\"search_intents\",\"query\":\"terraform vpc\",\"intent_id\":\"\",\"date_from\":\"\",\"date_to\":\"\",\"project\":\"\",\"limit\":5,\"answer\":\"\"}";

/// How the copilot talks to the model about tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolMode {
    /// Native function calling, falling back to the JSON action protocol when rejected.
    #[default]
    Auto,
    Native,
    Json,
}

#[derive(Debug, Clone)]
pub struct CopilotConfig {
    pub limits: ToolLimits,
    pub memory_capacity: usize,
    /// Per model turn (including retries).
    pub turn_timeout: Duration,
    pub tool_mode: ToolMode,
}

impl CopilotConfig {
    /// Local models get a longer per-turn budget.
    pub fn for_identity(identity: &crate::client::ClientIdentity) -> Self {
        let turn_timeout = if identity.is_local { Duration::from_secs(120) } else { Duration::from_secs(60) };
        Self { turn_timeout, ..Self::default() }
    }
}

impl Default for CopilotConfig {
    fn default() -> Self {
        Self {
            limits: ToolLimits::default(),
            memory_capacity: 128,
            turn_timeout: Duration::from_secs(60),
            tool_mode: ToolMode::Auto,
        }
    }
}

#[derive(Debug, Clone)]
struct Memory {
    summary: String,
    intent_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Action {
    action: String,
    #[serde(default)]
    query: String,
    #[serde(default)]
    intent_id: String,
    #[serde(default)]
    date_from: String,
    #[serde(default)]
    date_to: String,
    #[serde(default)]
    project: String,
    #[serde(default)]
    limit: i64,
    #[serde(default)]
    answer: String,
}

impl Action {
    fn arguments(&self) -> Value {
        let mut m = serde_json::Map::new();
        for (k, v) in [
            ("query", &self.query),
            ("intent_id", &self.intent_id),
            ("date_from", &self.date_from),
            ("date_to", &self.date_to),
            ("project", &self.project),
        ] {
            if !v.trim().is_empty() {
                m.insert(k.into(), json!(v.trim()));
            }
        }
        if self.limit > 0 && self.action == "search_intents" {
            m.insert("limit".into(), json!(self.limit));
        }
        Value::Object(m)
    }
}

fn action_schema() -> Value {
    let mut actions: Vec<&str> = tools::ALLOWED_TOOLS.to_vec();
    actions.push("answer");
    let props: BTreeMap<&str, Value> = [
        ("action", json!({"type": "string", "enum": actions})),
        ("query", json!({"type": "string"})),
        ("intent_id", json!({"type": "string"})),
        ("date_from", json!({"type": "string"})),
        ("date_to", json!({"type": "string"})),
        ("project", json!({"type": "string"})),
        ("limit", json!({"type": "integer"})),
        ("answer", json!({"type": "string"})),
    ]
    .into_iter()
    .collect();
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["action", "query", "intent_id", "date_from", "date_to", "project", "limit", "answer"],
        "properties": props,
    })
}

pub struct Copilot {
    harness: Arc<Harness>,
    backend: Arc<dyn CopilotBackend>,
    config: CopilotConfig,
    memory: Mutex<Lru<String, Memory>>,
}

struct RunState {
    messages: Vec<ChatMessage>,
    citations: BTreeMap<String, CitedIntent>,
    citation_order: Vec<String>,
    tool_names: Vec<String>,
    last_answer: Option<String>,
    resume: Option<ResumeProposal>,
    resume_candidate: Option<String>,
}

impl RunState {
    fn collect_citations(&mut self, result: &Value) {
        let mut records: Vec<&Value> = Vec::new();
        if let Some(intent) = result.get("intent").filter(|v| v.is_object()) {
            records.push(intent);
        }
        if let Some(list) = result.get("results").and_then(Value::as_array) {
            records.extend(list.iter());
        }
        for r in records {
            let id = r.get("id").or_else(|| r.get("intent_id")).and_then(Value::as_str);
            let date = r.get("date").and_then(Value::as_str);
            let (Some(id), Some(date)) = (id, date) else { continue };
            if !self.citations.contains_key(id) {
                self.citation_order.push(id.to_owned());
                self.citations.insert(
                    id.to_owned(),
                    CitedIntent {
                        intent_id: id.to_owned(),
                        date: date.to_owned(),
                        label: r.get("label").and_then(Value::as_str).unwrap_or_default().to_owned(),
                        summary: r.get("summary").and_then(Value::as_str).unwrap_or_default().to_owned(),
                    },
                );
            }
        }
    }

    fn push_verified(&mut self, name: &str, result: &Value) {
        let content = json!({"verified_tool_result": {name: model_tool_result(name, result)}}).to_string();
        self.messages.push(ChatMessage::user(content));
    }
}

fn new_conversation_id() -> String {
    let (a, b) = (fastrand::u64(..), fastrand::u64(..));
    let h = format!("{a:016x}{b:016x}");
    format!("{}-{}-4{}-a{}-{}", &h[0..8], &h[8..12], &h[13..16], &h[17..20], &h[20..32])
}

impl Copilot {
    pub fn new(harness: Arc<Harness>, backend: Arc<dyn CopilotBackend>, config: CopilotConfig) -> Self {
        let memory = Mutex::new(Lru::new(config.memory_capacity));
        Self { harness, backend, config, memory }
    }

    fn system_prompt(request: &CopilotQueryRequest) -> String {
        match request.mode {
            CopilotMode::Qa | CopilotMode::Auto => {
                let mut p = format!("{SYSTEM_PROMPT}{QA_PROMPT}");
                if request.mode == CopilotMode::Auto && request.intent_id.is_some() {
                    p.push_str(BRIEFING_PROMPT);
                }
                p
            }
            CopilotMode::Briefing => format!("{SYSTEM_PROMPT}{BRIEFING_PROMPT}"),
            CopilotMode::Narrative => format!("{SYSTEM_PROMPT}{NARRATIVE_PROMPT}"),
            CopilotMode::Search => SYSTEM_PROMPT.to_owned(),
        }
    }

    /// Request-level date bounds always override model-chosen bounds.
    fn with_request_filters(name: &str, arguments: &Value, request: &CopilotQueryRequest) -> Value {
        let mut args = arguments.as_object().cloned().unwrap_or_default();
        if matches!(name, "search_intents" | "get_intent_stats") {
            if let Some(from) = &request.date_from {
                args.insert("date_from".into(), json!(from));
            }
            if let Some(to) = &request.date_to {
                args.insert("date_to".into(), json!(to));
            }
        }
        Value::Object(args)
    }

    pub async fn query(&self, request: &CopilotQueryRequest) -> Result<CopilotQueryResponse, LlmError> {
        let limits = self.config.limits;
        let mut session = ToolSession::new(self.backend.as_ref(), limits);
        let conversation_id = request.conversation_id.clone().unwrap_or_else(new_conversation_id);
        let mut st = RunState {
            messages: Vec::new(),
            citations: BTreeMap::new(),
            citation_order: Vec::new(),
            tool_names: Vec::new(),
            last_answer: None,
            resume: None,
            resume_candidate: request.intent_id.clone(),
        };
        let prior = self.memory.lock().ok().and_then(|mut m| m.get(&conversation_id));
        if let Some(prior) = prior {
            st.messages.push(ChatMessage::user(
                json!({
                    "prior_summary": truncate_chars(&redact_text(&prior.summary), 240),
                    "prior_intent_ids": prior.intent_ids.iter().take(20).map(|i| truncate_chars(i, 128)).collect::<Vec<_>>(),
                })
                .to_string(),
            ));
        }
        st.messages.push(ChatMessage::user(redact_text(truncate_chars(&request.question, 2000))));

        let briefing =
            request.mode == CopilotMode::Briefing || (request.mode == CopilotMode::Auto && request.intent_id.is_some());
        let narrative = request.mode == CopilotMode::Narrative;
        let mut briefing_target = if briefing { request.intent_id.clone() } else { None };
        let mut evidence_found = false;

        if briefing {
            let mut selected = request.intent_id.clone();
            if selected.is_none() {
                let args = Self::with_request_filters(
                    "search_intents",
                    &json!({"query": truncate_chars(&request.question, limits.max_query_chars), "limit": limits.max_results}),
                    request,
                );
                let result = session.execute("search_intents", &args).await;
                st.tool_names.push("search_intents".into());
                st.collect_citations(&result);
                let mut ids: Vec<&str> = Vec::new();
                for r in result.get("results").and_then(Value::as_array).into_iter().flatten() {
                    if let Some(id) = r.get("id").and_then(Value::as_str)
                        && !ids.contains(&id)
                    {
                        ids.push(id);
                    }
                }
                if ids.len() == 1 {
                    selected = Some(ids[0].to_owned());
                }
            }
            briefing_target = selected.clone();
            if let Some(id) = selected {
                st.resume_candidate = Some(id.clone());
                let intent_result = session.execute("get_intent", &json!({"intent_id": id})).await;
                st.tool_names.push("get_intent".into());
                st.collect_citations(&intent_result);
                st.push_verified("get_intent", &intent_result);
                let payload_result = session.execute("get_resume_payload", &json!({"intent_id": id})).await;
                st.tool_names.push("get_resume_payload".into());
                if let Ok(Some(payload)) = self.backend.get_resume_payload(&id).await {
                    st.resume = Some(ResumeProposal { intent_id: id.clone(), resume_payload: payload, briefing: None });
                }
                st.push_verified("get_resume_payload", &payload_result);
            }
        }

        if narrative && let (Some(from), Some(to)) = (&request.date_from, &request.date_to) {
            let result = session.execute("get_intent_stats", &json!({"date_from": from, "date_to": to})).await;
            st.tool_names.push("get_intent_stats".into());
            evidence_found = result.pointer("/stats/intent_count").and_then(Value::as_i64).unwrap_or(0) > 0;
            st.push_verified("get_intent_stats", &result);
        }

        if !briefing
            && matches!(request.mode, CopilotMode::Search | CopilotMode::Auto)
            && request.question.split_whitespace().count() > 3
        {
            let queries = rewrite_search_queries(&request.question, &self.harness, self.config.turn_timeout).await;
            let mut merged: Vec<Value> = Vec::new();
            for q in queries {
                if st.tool_names.len() >= limits.max_tool_calls {
                    break;
                }
                let args = Self::with_request_filters(
                    "search_intents",
                    &json!({"query": q, "limit": limits.max_results}),
                    request,
                );
                let result = session.execute("search_intents", &args).await;
                st.tool_names.push("search_intents".into());
                st.collect_citations(&result);
                for r in result.get("results").and_then(Value::as_array).into_iter().flatten() {
                    if let Some(id) = r.get("id").and_then(Value::as_str)
                        && !merged.iter().any(|m| m["id"] == id)
                    {
                        merged.push(json!({"id": id, "label": r.get("label"), "summary": r.get("summary"), "date": r.get("date")}));
                    }
                }
            }
            if !merged.is_empty() {
                st.push_verified("search_intents", &json!({"results": merged}));
            }
        }

        let system = Self::system_prompt(request);
        let mut use_native = match self.config.tool_mode {
            ToolMode::Native => true,
            ToolMode::Json => false,
            ToolMode::Auto => self.harness.client().supports_native_tools(),
        };
        let specs = tool_specs(&limits);
        let mut cap_reached = true;
        let mut iterations = 0;
        while iterations < limits.max_tool_calls {
            iterations += 1;
            let calls: Vec<ToolCall> = if use_native {
                match self.harness.tools_turn(&system, &st.messages, &specs, self.config.turn_timeout).await {
                    Ok(turn) => {
                        if let Some(text) = turn.text.as_deref().filter(|t| !t.is_empty()) {
                            st.last_answer = Some(truncate_chars(text, 4000).to_owned());
                        }
                        if !turn.tool_calls.is_empty() {
                            st.messages.push(ChatMessage::Assistant {
                                content: turn.text.clone(),
                                tool_calls: turn.tool_calls.clone(),
                            });
                        }
                        turn.tool_calls
                    }
                    Err(HarnessError::Provider(e))
                        if self.config.tool_mode == ToolMode::Auto
                            && e.error_code.as_deref() == Some(codes::TOOLS_UNSUPPORTED) =>
                    {
                        tracing::info!("model rejected native tools; switching copilot to the JSON action protocol");
                        use_native = false;
                        iterations -= 1;
                        continue;
                    }
                    Err(e) => return Err(e.into()),
                }
            } else {
                let action = self.next_action(&system, &st.messages).await?;
                if action.action == "answer" || !tools::ALLOWED_TOOLS.contains(&action.action.as_str()) {
                    if !action.answer.trim().is_empty() {
                        st.last_answer = Some(truncate_chars(action.answer.trim(), 4000).to_owned());
                    }
                    vec![]
                } else {
                    let call = ToolCall {
                        id: format!("action-{iterations}"),
                        name: action.action.clone(),
                        arguments: action.arguments(),
                        signature: None,
                    };
                    st.messages.push(ChatMessage::Assistant { content: None, tool_calls: vec![call.clone()] });
                    vec![call]
                }
            };
            if calls.is_empty() {
                cap_reached = false;
                break;
            }
            for call in calls {
                st.tool_names.push(call.name.clone());
                if call.name == "get_resume_payload" {
                    let requested = call.arguments.get("intent_id").and_then(Value::as_str).map(str::to_owned);
                    if !briefing || (briefing_target.is_some() && requested == briefing_target) {
                        st.resume_candidate = requested.or(st.resume_candidate.take());
                    }
                }
                let args = Self::with_request_filters(&call.name, &call.arguments, request);
                let result = session.execute(&call.name, &args).await;
                st.collect_citations(&result);
                if call.name == "get_resume_payload"
                    && result.get("resume_payload_available").and_then(Value::as_bool) == Some(true)
                    && let Some(candidate) = st.resume_candidate.clone()
                    && (!briefing
                        || (briefing_target.is_some()
                            && call.arguments.get("intent_id").and_then(Value::as_str) == briefing_target.as_deref()))
                    && let Ok(Some(payload)) = self.backend.get_resume_payload(&candidate).await
                {
                    st.resume = Some(ResumeProposal { intent_id: candidate, resume_payload: payload, briefing: None });
                }
                st.messages.push(ChatMessage::Tool {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    content: model_tool_result(&call.name, &result).to_string(),
                });
            }
        }

        let sufficient = (!st.citations.is_empty() || evidence_found) && (!briefing || st.resume.is_some());
        let answer = if briefing && !sufficient {
            "I found no single stored intent with a verified resume payload for this briefing.".to_owned()
        } else if narrative && !sufficient {
            "I found no stored statistics for this narrative.".to_owned()
        } else {
            st.last_answer.clone().unwrap_or_else(|| {
                if cap_reached {
                    "Tool-call limit reached before a complete answer could be produced.".into()
                } else if !sufficient {
                    "I found no stored intent evidence for this question.".into()
                } else {
                    "Evidence was found in stored intents.".into()
                }
            })
        };
        let summary = truncate_chars(&answer.replace('\n', " "), 240).to_owned();
        let cited_ids: Vec<String> = st.citation_order.iter().take(20).cloned().collect();
        if let Ok(mut m) = self.memory.lock() {
            m.put(conversation_id.clone(), Memory { summary: summary.clone(), intent_ids: cited_ids });
        }
        let resume_proposal = if briefing {
            st.resume.filter(|_| sufficient).map(|r| ResumeProposal { briefing: Some(answer.clone()), ..r })
        } else {
            st.resume
        };
        let citations = st.citation_order.iter().filter_map(|id| st.citations.get(id).cloned()).collect();
        Ok(CopilotQueryResponse {
            answer,
            citations,
            confidence: if sufficient { 0.8 } else { 0.0 },
            evidence_status: if sufficient { EvidenceStatus::Sufficient } else { EvidenceStatus::Insufficient },
            resume_proposal,
            tool_calls_made: st.tool_names,
            conversation_id: Some(conversation_id),
            cached_summary: Some(summary),
        })
    }

    async fn next_action(&self, system: &str, messages: &[ChatMessage]) -> Result<Action, LlmError> {
        let transcript: Vec<String> = messages
            .iter()
            .map(|m| match m {
                ChatMessage::User { content } => format!("USER: {content}"),
                ChatMessage::Assistant { content, tool_calls } => {
                    let calls: Vec<String> =
                        tool_calls.iter().map(|c| format!("{}({})", c.name, c.arguments)).collect();
                    format!("ASSISTANT: {}{}", content.as_deref().unwrap_or_default(), calls.join("; "))
                }
                ChatMessage::Tool { name, content, .. } => format!("TOOL RESULT {name}: {content}"),
            })
            .collect();
        let mut call = StructuredCall::new(
            "copilot-action",
            "copilot-action-2.0-1",
            format!("{system}{ACTION_PROTOCOL}"),
            transcript.join("\n"),
            "copilot_action",
            action_schema(),
        );
        call.mode = JsonMode::Schema;
        call.max_output_tokens = 800;
        call.deadline = Some(self.config.turn_timeout);
        call.use_cache = false;
        let result = self
            .harness
            .structured::<Action, _>(call, |a| {
                if a.action == "answer" || tools::ALLOWED_TOOLS.contains(&a.action.as_str()) {
                    Ok(())
                } else {
                    Err(format!("action must be one of {:?} or answer", tools::ALLOWED_TOOLS))
                }
            })
            .await?;
        Ok(result.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_ids_look_like_uuids() {
        let id = new_conversation_id();
        assert_eq!(id.len(), 36);
        assert_eq!(id.matches('-').count(), 4);
    }

    #[test]
    fn request_filters_override_model_dates() {
        let req = CopilotQueryRequest {
            question: "q".into(),
            date_from: Some("2024-01-01".into()),
            date_to: None,
            mode: CopilotMode::Qa,
            intent_id: None,
            conversation_id: None,
        };
        let out =
            Copilot::with_request_filters("search_intents", &json!({"query": "x", "date_from": "1999-01-01"}), &req);
        assert_eq!(out["date_from"], "2024-01-01");
        let untouched = Copilot::with_request_filters("get_intent", &json!({"intent_id": "a", "date_from": "x"}), &req);
        assert_eq!(untouched["date_from"], "x");
    }

    #[test]
    fn action_arguments_drop_empty_fields() {
        let a = Action {
            action: "search_intents".into(),
            query: "vpc".into(),
            intent_id: "".into(),
            date_from: "".into(),
            date_to: "".into(),
            project: "".into(),
            limit: 3,
            answer: "".into(),
        };
        assert_eq!(a.arguments(), json!({"query": "vpc", "limit": 3}));
    }
}
