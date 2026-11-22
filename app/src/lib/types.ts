// TypeScript mirror of crates/intent-types (the frozen JSON contract) plus the
// capture-side /v1/* shapes the UI consumes. Keep field names identical to serde.

export interface ShellResume {
  cwd?: string | null;
  last_cmd?: string | null;
}

export interface ResumePayload {
  files: string[];
  urls: string[];
  shell: ShellResume;
}

export interface IntentStats {
  event_count: number;
  duration_seconds: number;
  sources: Record<string, number>;
  unique_apps: string[];
}

/** Insight rows are free-form maps on the wire; these are the keys we read. */
export interface InsightRow {
  file?: string;
  line?: number;
  domain?: string;
  title?: string;
  url?: string;
  command?: string;
  command_family?: string;
  count?: number;
  exit_code?: number;
  [key: string]: unknown;
}

export interface IntentInsights {
  editor: InsightRow[];
  browser: InsightRow[];
  shell: InsightRow[];
}

export interface TodoObservation {
  path: string;
  observed_ts: number;
  marker: string;
}

export interface ContextEvidence {
  field: string;
  value: string;
}

export interface SemanticIntentMetadata {
  refined: boolean;
  confidence: number | null;
  event_roles: Record<string, number>;
  topic: string | null;
  workspace_root: string | null;
  provider_identity: string | null;
}

export interface Intent {
  id: string;
  parent_id: string | null;
  date: string;
  label: string;
  summary: string;
  confidence: number;
  start_ts: number;
  end_ts: number;
  depth: number;
  tags: string[];
  stats: IntentStats;
  insights: IntentInsights;
  todos: TodoObservation[];
  evidence: ContextEvidence[];
  resume_payload: ResumePayload;
  prefix?: [string, string, string] | null;
  semantic?: SemanticIntentMetadata | null;
  privacy_policy_version: string;
  children: Intent[];
}

export interface CurrentIntent {
  label: string;
  summary: string;
  confidence: number;
  since_ts: number;
}

export interface Digest {
  date: string;
  headline: string;
  summary: string;
  top_intent_ids: string[];
  intent_count: number;
  total_duration_seconds: number;
}

export interface SearchHit {
  id: string;
  label: string;
  summary: string;
  date: string;
  highlight_snippet: string;
  score: number;
}

export interface CitedIntent {
  intent_id: string;
  date: string;
  label: string;
  summary: string;
}

export interface ResumeProposal {
  intent_id: string;
  resume_payload: ResumePayload;
  briefing: string | null;
}

export interface ResumeSelectCandidate {
  intent_id: string;
  label: string;
  summary: string;
  project_tag: string | null;
  workspace_root: string | null;
  score: number;
}

export interface ResumeSelectPreview extends ResumeSelectCandidate {
  resume_payload: ResumePayload;
}

export interface ResumeSelectResponse {
  needs_picker: boolean;
  candidates: ResumeSelectCandidate[];
  selected: ResumeSelectPreview | null;
}

export interface ResumeSelectRequest {
  intent_id?: string | null;
  project_tag?: string | null;
  query?: string | null;
  restore_scope?: 'same_project' | null;
}

export type CopilotMode = 'auto' | 'search' | 'qa' | 'briefing' | 'narrative';

export interface CopilotQueryRequest {
  question: string;
  date_from?: string | null;
  date_to?: string | null;
  mode?: CopilotMode;
  intent_id?: string | null;
  conversation_id?: string | null;
}

export interface CopilotQueryResponse {
  answer: string;
  citations: CitedIntent[];
  confidence: number;
  evidence_status: 'sufficient' | 'insufficient';
  resume_proposal: ResumeProposal | null;
  tool_calls_made: string[];
  conversation_id: string | null;
  cached_summary: string | null;
}

export type LlmProvider = 'local' | 'openai' | 'groq' | 'gemini' | 'bedrock' | 'none';

export interface LlmSettingsSummary {
  provider: LlmProvider;
  model: string | null;
  base_url: string | null;
  embedding_model: string | null;
  copilot_enabled: boolean;
  semantic_cluster_enabled: boolean;
  semantic_content_consent: boolean;
  semantic_full_capture_consent: boolean;
  semantic_timeout_ms: number;
  api_key_configured: boolean;
  credentials_configured: boolean;
  google_cloud_project: string | null;
  google_cloud_location: string | null;
  bedrock_region: string | null;
  bedrock_profile: string | null;
  local_server_reachable: boolean | null;
}

export interface LlmSettingsUpdate {
  provider: LlmProvider;
  api_key?: string | null;
  clear_api_key?: boolean;
  model?: string | null;
  base_url?: string | null;
  embedding_model?: string | null;
  enable_copilot?: boolean | null;
  enable_semantic_cluster?: boolean | null;
  enable_semantic_content_consent?: boolean | null;
  enable_semantic_full_capture_consent?: boolean | null;
  semantic_timeout_ms?: number | null;
  google_cloud_project?: string | null;
  google_cloud_location?: string | null;
  bedrock_region?: string | null;
  bedrock_profile?: string | null;
}

// ---------------------------------------------------------------------------
// Capture side (/v1/*)
// ---------------------------------------------------------------------------

export type CaptureSource = 'vscode' | 'firefox' | 'chrome' | 'shell' | 'linux' | 'filesystem';

export interface SourceStatus {
  event_count: number;
  last_event_ts: number | null;
  last_ingested_at: number | null;
  last_ingest_lag_seconds: number | null;
  healthy: boolean;
}

export interface DetailedCaptureConfig {
  editor: { enabled: boolean; excluded_patterns: string[] };
  browser: { enabled: boolean; context_enabled: boolean };
  filesystem: { enabled: boolean };
  approved_workspaces: string[];
  retention: string;
  export_includes_details: boolean;
}

export interface DetailedCaptureUpdate {
  editor?: { enabled?: boolean; excluded_patterns?: string[] };
  browser?: { enabled?: boolean; context_enabled?: boolean };
  filesystem?: { enabled?: boolean };
}

export interface CaptureStatus {
  ok: boolean;
  session_type: string;
  capture_paused: boolean;
  /**
   * Epoch seconds when a timed pause ends. 2.0 addition: intentd accepts
   * `{"paused": true, "until": <epoch>}` on /v1/capture/pause and echoes it here.
   */
  capture_paused_until?: number | null;
  sources: Record<string, SourceStatus>;
  services?: Record<string, boolean>;
  detailed_capture?: DetailedCaptureConfig & { event_counts: Record<string, number> };
  /** 2.0 addition: on-disk database size in bytes. */
  db_size_bytes?: number | null;
  /**
   * 2.0 addition: problems in the config files that intentd works around (an
   * invalid blocked-domains entry, a refused workspace), in words for the user.
   */
  config_warnings?: string[];
}

export interface WorkspacesConfig {
  workspaces: string[];
}

export interface RetentionPolicy {
  metadata_days: number | null;
  detailed_days: number | null;
}

export interface RetentionPreview {
  now: number;
  detailed_days: number | null;
  metadata_days: number | null;
  /** `total` counts events; `intents` counts threads (roots and children) of days past the cutoff. */
  eligible: { detailed: number; metadata: number; total: number; intents: number };
}

export interface RestoreRequest {
  mode: 'resume' | 'continue';
  files: string[];
  urls: string[];
  shell: { cwd?: string | null; last_cmd?: string | null };
}

export interface RestoreResult {
  ok: boolean;
  restored: { files: number; urls: number; shell: boolean };
  failed: string[];
}

export interface BlockedDomainsConfig {
  blocked_domains: string[];
}

export interface HealthResponse {
  ok: boolean;
  version?: string;
}

export interface IntentContextResponse {
  intent_id: string;
  markdown: string;
}

export interface ForgetResponse {
  deleted_intent_ids: string[];
  deleted_count: number;
  deleted_event_count: number;
  /** Dates whose threads were removed; a forgotten project's dates are rebuilt. */
  affected_dates: string[];
}
