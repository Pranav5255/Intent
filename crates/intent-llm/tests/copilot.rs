//! Copilot loop with native tools, the JSON action fallback, and the automatic switch.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use intent_llm::client::{ChatMessage, ToolCall, ToolTurn, codes};
use intent_llm::copilot::{Copilot, CopilotBackend, CopilotConfig, IntentStatsReport, ToolMode};
use intent_llm::harness::Harness;
use intent_llm::mock::ScriptedClient;
use intent_llm::traits::LlmError;
use intent_types::{
    CopilotMode, CopilotQueryRequest, CurrentIntent, EvidenceStatus, Intent, IntentStats, ResumePayload,
    SAFE_INTENT_PRIVACY_POLICY_VERSION, SearchHit, ShellResume,
};
use serde_json::json;

/// (query, date_from, date_to)
type Search = (String, Option<String>, Option<String>);

#[derive(Default)]
struct Backend {
    searches: Mutex<Vec<Search>>,
}

fn intent(id: &str) -> Intent {
    Intent {
        id: id.into(),
        parent_id: None,
        date: "2024-05-02".into(),
        label: "Terraform VPC work".into(),
        summary: "Planned VPC changes.".into(),
        confidence: 0.8,
        start_ts: 1,
        end_ts: 2,
        depth: 0,
        tags: vec!["project:infra".into()],
        stats: IntentStats { event_count: 4, duration_seconds: 60, ..Default::default() },
        insights: Default::default(),
        todos: vec![],
        evidence: vec![],
        resume_payload: ResumePayload {
            files: vec!["/home/u/infra/main.tf".into()],
            urls: vec!["https://registry.terraform.io/".into()],
            shell: ShellResume { cwd: Some("/home/u/infra".into()), last_cmd: Some("terraform plan".into()) },
        },
        prefix: None,
        semantic: None,
        privacy_policy_version: SAFE_INTENT_PRIVACY_POLICY_VERSION.into(),
        children: vec![],
    }
}

#[async_trait]
impl CopilotBackend for Backend {
    async fn search_intents(
        &self,
        query: &str,
        _: usize,
        from: Option<&str>,
        to: Option<&str>,
    ) -> Result<Vec<SearchHit>, String> {
        self.searches.lock().unwrap().push((query.into(), from.map(Into::into), to.map(Into::into)));
        Ok(vec![SearchHit {
            id: "i1".into(),
            label: "x".into(),
            summary: "y".into(),
            date: "2024-05-02".into(),
            highlight_snippet: "secret snippet".into(),
            score: 1.0,
        }])
    }
    async fn get_intent(&self, id: &str) -> Result<Option<Intent>, String> {
        Ok((id == "i1").then(|| intent(id)))
    }
    async fn get_current_intent(&self) -> Result<Option<CurrentIntent>, String> {
        Ok(None)
    }
    async fn get_intent_stats(&self, from: &str, to: &str, _: Option<&str>) -> Result<IntentStatsReport, String> {
        Ok(IntentStatsReport {
            date_from: Some(from.into()),
            date_to: Some(to.into()),
            intent_count: 3,
            ..Default::default()
        })
    }
}

fn request(question: &str, mode: CopilotMode) -> CopilotQueryRequest {
    CopilotQueryRequest {
        question: question.into(),
        date_from: Some("2024-05-01".into()),
        date_to: Some("2024-05-03".into()),
        mode,
        intent_id: None,
        conversation_id: None,
    }
}

#[tokio::test]
async fn native_tool_loop_cites_and_strips_private_fields() {
    let client = Arc::new(ScriptedClient::new(true));
    client.push_tools(Ok(ToolTurn {
        text: None,
        tool_calls: vec![ToolCall {
            id: "c1".into(),
            name: "search_intents".into(),
            arguments: json!({"query": "vpc", "date_from": "1999-01-01"}),
            signature: None,
        }],
    }));
    client.push_tools(Ok(ToolTurn {
        text: None,
        tool_calls: vec![ToolCall {
            id: "c2".into(),
            name: "get_resume_payload".into(),
            arguments: json!({"intent_id": "i1"}),
            signature: None,
        }],
    }));
    client.push_tools(Ok(ToolTurn { text: Some("You worked on the VPC (i1).".into()), tool_calls: vec![] }));
    let backend = Arc::new(Backend::default());
    let copilot =
        Copilot::new(Arc::new(Harness::for_client(client.clone())), backend.clone(), CopilotConfig::default());

    let resp = copilot.query(&request("vpc?", CopilotMode::Qa)).await.unwrap();
    assert_eq!(resp.answer, "You worked on the VPC (i1).");
    assert_eq!(resp.evidence_status, EvidenceStatus::Sufficient);
    assert_eq!(resp.citations.len(), 1);
    assert_eq!(resp.citations[0].label, "Terraform VPC work");
    assert_eq!(resp.tool_calls_made, vec!["search_intents", "get_resume_payload"]);
    let proposal = resp.resume_proposal.expect("resume proposal assembled locally");
    assert_eq!(proposal.resume_payload.files, vec!["/home/u/infra/main.tf"]);
    // Request date bounds override the model's.
    assert_eq!(backend.searches.lock().unwrap()[0].1.as_deref(), Some("2024-05-01"));

    let sent = client.tool_requests.lock().unwrap();
    let last = format!("{:?}", sent.last().unwrap());
    assert!(!last.contains("main.tf"), "restore data never reaches the model");
    assert!(!last.contains("secret snippet"));
    assert!(last.contains("file_count"));
    assert!(matches!(sent.last().unwrap().last().unwrap(), ChatMessage::Tool { .. }));
}

#[tokio::test]
async fn json_action_protocol_for_models_without_tools() {
    let client = Arc::new(ScriptedClient::new(true).without_native_tools());
    client.push_json(Ok(
        r#"{"action":"get_intent","query":"","intent_id":"i1","date_from":"","date_to":"","project":"","limit":0,"answer":""}"#.into(),
    ));
    client.push_json(Ok(
        "```json\n{\"action\":\"answer\",\"query\":\"\",\"intent_id\":\"\",\"date_from\":\"\",\"date_to\":\"\",\"project\":\"\",\"limit\":0,\"answer\":\"It was VPC work.\"}\n```".into(),
    ));
    let copilot = Copilot::new(
        Arc::new(Harness::for_client(client.clone())),
        Arc::new(Backend::default()),
        CopilotConfig::default(),
    );
    let resp = copilot.query(&request("vpc?", CopilotMode::Qa)).await.unwrap();
    assert_eq!(resp.answer, "It was VPC work.");
    assert_eq!(resp.tool_calls_made, vec!["get_intent"]);
    assert_eq!(resp.citations[0].intent_id, "i1");
    let second = client.requests.lock().unwrap()[1].user.clone();
    assert!(second.contains("TOOL RESULT get_intent"));
    assert!(!second.contains("main.tf"));
    assert!(client.tool_requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn auto_mode_switches_when_tools_are_rejected() {
    let client = Arc::new(ScriptedClient::new(true));
    client.push_tools(Err(LlmError::with_code("model does not support tools", codes::TOOLS_UNSUPPORTED)));
    client.push_json(Ok(
        r#"{"action":"answer","query":"","intent_id":"","date_from":"","date_to":"","project":"","limit":0,"answer":"No evidence."}"#.into(),
    ));
    let config = CopilotConfig { tool_mode: ToolMode::Auto, ..Default::default() };
    let copilot = Copilot::new(Arc::new(Harness::for_client(client.clone())), Arc::new(Backend::default()), config);
    let resp = copilot.query(&request("hi", CopilotMode::Qa)).await.unwrap();
    assert_eq!(resp.answer, "No evidence.");
    assert_eq!(resp.evidence_status, EvidenceStatus::Insufficient);
}

#[tokio::test]
async fn search_mode_rewrites_queries_and_remembers_conversation() {
    let client = Arc::new(ScriptedClient::new(true));
    client.push_json(Ok(r#"{"queries":["terraform vpc","aws networking"]}"#.into()));
    client.push_tools(Ok(ToolTurn { text: Some("Found it.".into()), tool_calls: vec![] }));
    client.push_tools(Ok(ToolTurn { text: Some("Again.".into()), tool_calls: vec![] }));
    let backend = Arc::new(Backend::default());
    let copilot =
        Copilot::new(Arc::new(Harness::for_client(client.clone())), backend.clone(), CopilotConfig::default());
    let mut req = request("what was I doing with the vpc", CopilotMode::Search);
    let resp = copilot.query(&req).await.unwrap();
    assert_eq!(resp.tool_calls_made, vec!["search_intents", "search_intents"]);
    let queries: Vec<String> = backend.searches.lock().unwrap().iter().map(|s| s.0.clone()).collect();
    assert_eq!(queries, vec!["terraform vpc", "aws networking"]);

    req.conversation_id = resp.conversation_id.clone();
    req.question = "and then?".into();
    copilot.query(&req).await.unwrap();
    let sent = client.tool_requests.lock().unwrap();
    assert!(format!("{:?}", sent.last().unwrap()[0]).contains("prior_summary"));
}

#[tokio::test]
async fn briefing_requires_verified_resume_payload() {
    let client = Arc::new(ScriptedClient::new(true));
    client.push_tools(Ok(ToolTurn { text: Some("Resume the VPC plan in i1.".into()), tool_calls: vec![] }));
    let copilot = Copilot::new(
        Arc::new(Harness::for_client(client.clone())),
        Arc::new(Backend::default()),
        CopilotConfig::default(),
    );
    let mut req = request("brief me", CopilotMode::Briefing);
    req.intent_id = Some("i1".into());
    let resp = copilot.query(&req).await.unwrap();
    let proposal = resp.resume_proposal.unwrap();
    assert_eq!(proposal.briefing.as_deref(), Some("Resume the VPC plan in i1."));
    assert_eq!(resp.tool_calls_made, vec!["get_intent", "get_resume_payload"]);

    let mut missing = request("brief me", CopilotMode::Briefing);
    missing.intent_id = Some("nope".into());
    client.push_tools(Ok(ToolTurn { text: Some("made up".into()), tool_calls: vec![] }));
    let resp = copilot.query(&missing).await.unwrap();
    assert!(resp.resume_proposal.is_none());
    assert_eq!(resp.evidence_status, EvidenceStatus::Insufficient);
}

#[tokio::test]
async fn tool_call_cap_is_enforced() {
    let client = Arc::new(ScriptedClient::new(true));
    for i in 0..10 {
        client.push_tools(Ok(ToolTurn {
            text: None,
            tool_calls: vec![ToolCall {
                id: format!("c{i}"),
                name: "get_current_intent".into(),
                arguments: json!({}),
                signature: None,
            }],
        }));
    }
    let copilot = Copilot::new(
        Arc::new(Harness::for_client(client.clone())),
        Arc::new(Backend::default()),
        CopilotConfig::default(),
    );
    let resp = copilot.query(&request("loop", CopilotMode::Qa)).await.unwrap();
    assert_eq!(resp.tool_calls_made.len(), 8);
    assert!(resp.answer.contains("Tool-call limit"));
}
