//! Wire and pipeline contracts shared by every Intent crate and the Tauri app.
//!
//! Field names and JSON shapes match the 1.x Python services so the Firefox and
//! VS Code companions keep working unchanged against `intentd`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const DEFAULT_PORT: u16 = 9477;
pub const SAFE_INTENT_PRIVACY_POLICY_VERSION: &str = "safe-intent-features-v2";

pub type Payload = Map<String, Value>;

// ---------------------------------------------------------------------------
// Capture events
// ---------------------------------------------------------------------------

/// An event as stored and exported by the capture side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawEvent {
    pub id: String,
    pub ts: i64,
    pub source: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub payload: Payload,
    #[serde(default = "one")]
    pub schema_version: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ingested_at: Option<i64>,
}

fn one() -> i64 {
    1
}

/// `POST /v1/event` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestResult {
    pub ok: bool,
    pub inserted: bool,
    pub event: RawEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayExport {
    #[serde(default = "one")]
    pub version: i64,
    pub date: String,
    pub exported_at: i64,
    #[serde(default)]
    pub events: Vec<RawEvent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DayExportMeta {
    #[serde(default = "one")]
    pub version: i64,
    pub date: String,
    pub event_count: i64,
    pub first_ts: Option<i64>,
    pub last_ts: Option<i64>,
    pub last_event_id: Option<String>,
    #[serde(default)]
    pub revision: i64,
    pub content_hash: String,
    pub checked_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventCursor {
    pub after_ts: i64,
    pub after_id: String,
}

// ---------------------------------------------------------------------------
// Normalized pipeline events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    Editor,
    Browser,
    Command,
    Focus,
    FileChange,
    Idle,
    Other,
}

impl Family {
    pub fn as_str(self) -> &'static str {
        match self {
            Family::Editor => "editor",
            Family::Browser => "browser",
            Family::Command => "command",
            Family::Focus => "focus",
            Family::FileChange => "file_change",
            Family::Idle => "idle",
            Family::Other => "other",
        }
    }

    /// Single source of truth for `source`/`type` → family (was duplicated 3x in Python).
    pub fn from_source(source: &str, kind: &str) -> Family {
        match source {
            "vscode" => Family::Editor,
            "firefox" | "chrome" => Family::Browser,
            "linux" if kind.starts_with("idle_") => Family::Idle,
            "linux" => Family::Focus,
            "shell" => Family::Command,
            "filesystem" => Family::FileChange,
            _ => Family::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Code,
    Pdf,
    Image,
    Other,
}

/// Commands whose first token is recognised as a workflow family.
pub const COMMAND_FAMILIES: &[&str] =
    &["terraform", "git", "pytest", "python", "npm", "pip", "docker", "make", "cargo", "go", "pnpm", "yarn", "kubectl"];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EventEntities {
    #[serde(default)]
    pub project_paths: Vec<String>,
    pub file_path: Option<String>,
    pub file_name: Option<String>,
    pub file_kind: Option<FileKind>,
    pub domain: Option<String>,
    pub title: Option<String>,
    pub command: Option<String>,
    pub command_family: Option<String>,
    pub cwd: Option<String>,
    pub exit_code: Option<i64>,
    #[serde(default)]
    pub context_terms: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EventSignals {
    #[serde(default)]
    pub typed_chars: i64,
    #[serde(default)]
    pub save: bool,
    #[serde(default)]
    pub todo_added: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContextEvidence {
    pub field: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedEvent {
    pub id: String,
    pub ts: i64,
    pub ordinal: usize,
    pub source: String,
    pub family: Family,
    pub category: String,
    pub text: String,
    #[serde(default)]
    pub entities: EventEntities,
    #[serde(default)]
    pub signals: EventSignals,
    #[serde(default)]
    pub evidence: Vec<ContextEvidence>,
    #[serde(default)]
    pub raw: Payload,
}

// ---------------------------------------------------------------------------
// Intents
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ShellResume {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_cmd: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResumePayload {
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub urls: Vec<String>,
    #[serde(default)]
    pub shell: ShellResume,
}

pub const RESUME_MAX_FILES: usize = 5;
pub const RESUME_MAX_URLS: usize = 8;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IntentStats {
    pub event_count: i64,
    pub duration_seconds: i64,
    #[serde(default)]
    pub sources: BTreeMap<String, i64>,
    #[serde(default)]
    pub unique_apps: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct IntentInsights {
    #[serde(default)]
    pub editor: Vec<Map<String, Value>>,
    #[serde(default)]
    pub browser: Vec<Map<String, Value>>,
    #[serde(default)]
    pub shell: Vec<Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TodoObservation {
    pub path: String,
    pub observed_ts: i64,
    /// One of TODO, FIXME, XXX.
    pub marker: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SemanticIntentMetadata {
    #[serde(default = "yes")]
    pub refined: bool,
    pub confidence: Option<f64>,
    #[serde(default)]
    pub event_roles: BTreeMap<String, i64>,
    pub topic: Option<String>,
    pub workspace_root: Option<String>,
    pub provider_identity: Option<String>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Intent {
    pub id: String,
    pub parent_id: Option<String>,
    pub date: String,
    pub label: String,
    pub summary: String,
    #[serde(default)]
    pub confidence: f64,
    pub start_ts: i64,
    pub end_ts: i64,
    pub depth: i64,
    #[serde(default)]
    pub tags: Vec<String>,
    pub stats: IntentStats,
    #[serde(default)]
    pub insights: IntentInsights,
    #[serde(default)]
    pub todos: Vec<TodoObservation>,
    #[serde(default)]
    pub evidence: Vec<ContextEvidence>,
    #[serde(default)]
    pub resume_payload: ResumePayload,
    #[serde(default)]
    pub prefix: Option<(String, String, String)>,
    #[serde(default)]
    pub semantic: Option<SemanticIntentMetadata>,
    #[serde(default = "legacy")]
    pub privacy_policy_version: String,
    #[serde(default)]
    pub children: Vec<Intent>,
}

fn legacy() -> String {
    "legacy".into()
}

impl Intent {
    /// `project:<name>` tag value, if any.
    pub fn project_tag(&self) -> Option<&str> {
        self.tags.iter().find_map(|t| t.strip_prefix("project:"))
    }

    /// Depth-first iterator over this intent and all descendants.
    pub fn walk(&self) -> Vec<&Intent> {
        let mut out = vec![self];
        for child in &self.children {
            out.extend(child.walk());
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarningLevel {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineWarning {
    pub level: WarningLevel,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineResult {
    #[serde(default)]
    pub intents: Vec<Intent>,
    #[serde(default)]
    pub warnings: Vec<PipelineWarning>,
    pub source_hash: String,
    pub pipeline_version: String,
    #[serde(default)]
    pub cached: bool,
}

/// The only aggregate packet allowed to reach a (possibly remote) label provider.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SafeIntentFeatures {
    pub policy_version: String,
    pub project_key: Option<String>,
    #[serde(default)]
    pub command_families: Vec<String>,
    #[serde(default)]
    pub file_kinds: Vec<FileKind>,
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub file_names: Vec<String>,
    pub dominant_family: Option<Family>,
    pub semantic_topic: Option<String>,
    #[serde(default)]
    pub event_counts: BTreeMap<String, i64>,
    #[serde(default)]
    pub duration_seconds: i64,
    #[serde(default)]
    pub child_count: i64,
    #[serde(default)]
    pub boundary_reasons: Vec<String>,
}

// ---------------------------------------------------------------------------
// Semantic proposals (LLM structured output)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticRole {
    Task,
    SupportingContext,
    Background,
    Unrelated,
}

impl SemanticRole {
    pub fn is_work(self) -> bool {
        matches!(self, SemanticRole::Task | SemanticRole::SupportingContext)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticEventProposal {
    pub event_id: String,
    pub role: SemanticRole,
    pub confidence: f64,
    pub topic: String,
    #[serde(default)]
    pub linked_event_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticProposalResponse {
    pub proposals: Vec<SemanticEventProposal>,
}

// ---------------------------------------------------------------------------
// Engine API DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurrentIntent {
    pub label: String,
    pub summary: String,
    pub confidence: f64,
    pub since_ts: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PredictionResponse {
    pub predicted_label: String,
    pub confidence: f64,
    pub resume_payload: ResumePayload,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Digest {
    pub date: String,
    pub headline: String,
    pub summary: String,
    #[serde(default)]
    pub top_intent_ids: Vec<String>,
    pub intent_count: i64,
    pub total_duration_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub id: String,
    pub label: String,
    pub summary: String,
    pub date: String,
    pub highlight_snippet: String,
    /// Combined keyword + semantic score (new in 2.0; ignored by 1.x clients).
    #[serde(default)]
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CitedIntent {
    pub intent_id: String,
    pub date: String,
    pub label: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResumeProposal {
    pub intent_id: String,
    pub resume_payload: ResumePayload,
    pub briefing: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreScope {
    SameProject,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResumeSelectRequest {
    pub intent_id: Option<String>,
    pub project_tag: Option<String>,
    pub query: Option<String>,
    pub restore_scope: Option<RestoreScope>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResumeSelectCandidate {
    pub intent_id: String,
    pub label: String,
    pub summary: String,
    pub project_tag: Option<String>,
    pub workspace_root: Option<String>,
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResumeSelectPreview {
    #[serde(flatten)]
    pub candidate: ResumeSelectCandidate,
    pub resume_payload: ResumePayload,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResumeSelectResponse {
    pub needs_picker: bool,
    #[serde(default)]
    pub candidates: Vec<ResumeSelectCandidate>,
    pub selected: Option<ResumeSelectPreview>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopilotMode {
    #[default]
    Auto,
    Search,
    Qa,
    Briefing,
    Narrative,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CopilotQueryRequest {
    pub question: String,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    #[serde(default)]
    pub mode: CopilotMode,
    pub intent_id: Option<String>,
    pub conversation_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStatus {
    Sufficient,
    Insufficient,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CopilotQueryResponse {
    pub answer: String,
    #[serde(default)]
    pub citations: Vec<CitedIntent>,
    pub confidence: f64,
    pub evidence_status: EvidenceStatus,
    pub resume_proposal: Option<ResumeProposal>,
    #[serde(default)]
    pub tool_calls_made: Vec<String>,
    pub conversation_id: Option<String>,
    pub cached_summary: Option<String>,
}

// ---------------------------------------------------------------------------
// LLM settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmProvider {
    /// Ollama, llama.cpp `llama-server`, LM Studio, vLLM — any OpenAI-compatible local server.
    #[default]
    Local,
    Openai,
    Groq,
    Gemini,
    Bedrock,
    /// No LLM: rules + embeddings only.
    None,
}

/// What the UI sees; secrets are reduced to `*_configured` booleans.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LlmSettingsSummary {
    pub provider: LlmProvider,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub embedding_model: Option<String>,
    pub copilot_enabled: bool,
    pub semantic_cluster_enabled: bool,
    pub semantic_content_consent: bool,
    pub semantic_full_capture_consent: bool,
    pub semantic_timeout_ms: i64,
    pub api_key_configured: bool,
    pub credentials_configured: bool,
    pub google_cloud_project: Option<String>,
    pub google_cloud_location: Option<String>,
    pub bedrock_region: Option<String>,
    pub bedrock_profile: Option<String>,
    /// Result of the last reachability probe for local servers.
    pub local_server_reachable: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmSettingsUpdate {
    pub provider: LlmProvider,
    pub api_key: Option<String>,
    #[serde(default)]
    pub clear_api_key: bool,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub embedding_model: Option<String>,
    pub enable_copilot: Option<bool>,
    pub enable_semantic_cluster: Option<bool>,
    pub enable_semantic_content_consent: Option<bool>,
    pub enable_semantic_full_capture_consent: Option<bool>,
    pub semantic_timeout_ms: Option<i64>,
    pub google_cloud_project: Option<String>,
    pub google_cloud_location: Option<String>,
    pub bedrock_region: Option<String>,
    pub bedrock_profile: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_event_round_trips_python_shape() {
        let json =
            r#"{"id":"a","ts":1,"source":"shell","type":"command","payload":{"cmd":"ls","cwd":"/","exit_code":0}}"#;
        let ev: RawEvent = serde_json::from_str(json).unwrap();
        assert_eq!(ev.kind, "command");
        assert_eq!(ev.schema_version, 1);
        let back = serde_json::to_value(&ev).unwrap();
        assert_eq!(back["type"], "command");
        assert!(back.get("ingested_at").is_none());
    }

    #[test]
    fn family_mapping() {
        assert_eq!(Family::from_source("linux", "idle_start"), Family::Idle);
        assert_eq!(Family::from_source("linux", "app_focus"), Family::Focus);
        assert_eq!(Family::from_source("chrome", "tab_change"), Family::Browser);
        assert_eq!(serde_json::to_string(&Family::FileChange).unwrap(), "\"file_change\"");
    }

    #[test]
    fn semantic_response_rejects_unknown_fields() {
        let bad = r#"{"proposals":[],"extra":1}"#;
        assert!(serde_json::from_str::<SemanticProposalResponse>(bad).is_err());
    }
}
