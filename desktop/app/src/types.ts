export interface ResumePayload {
  files: string[];
  urls: string[];
  shell: {
    cwd?: string;
    last_cmd?: string;
  };
}

export interface IntentStats {
  event_count: number;
  duration_seconds: number;
  sources: Record<string, number>;
  unique_apps?: string[];
}

export interface IntentInsight {
  file?: string;
  domain?: string;
  command_family?: string;
  count?: number;
  exit_code?: number;
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
  insights: {
    editor: IntentInsight[];
    browser: IntentInsight[];
    shell: IntentInsight[];
  };
  todos: Array<{ path: string; observed_ts: number; marker: string }>;
  resume_payload: ResumePayload;
  children: Intent[];
}

export interface Digest {
  date: string;
  headline: string;
  summary: string;
  top_intent_ids: string[];
  intent_count: number;
  total_duration_seconds: number;
}

export interface CurrentIntent {
  label: string;
  summary: string;
  confidence: number;
  since_ts: number;
}

export interface SearchResult {
  id: string;
  label: string;
  summary: string;
  date: string;
  highlight_snippet: string;
}

export interface ResumeCandidate {
  intent_id: string;
  label: string;
  summary: string;
  project_tag: string | null;
  workspace_root: string | null;
  score: number;
}

export interface ResumePreview extends ResumeCandidate {
  resume_payload: ResumePayload;
}

export interface ResumeSelectResponse {
  needs_picker: boolean;
  candidates: ResumeCandidate[];
  selected: ResumePreview | null;
}

export interface RestoreResult {
  ok: boolean;
  restored: { files: number; urls: number; shell: boolean };
  failed: string[];
}

export interface CopilotCitation {
  intent_id: string;
  date: string;
  label: string;
  summary: string;
}

export interface CopilotResponse {
  answer: string;
  citations: CopilotCitation[];
  confidence: number;
  evidence_status: 'sufficient' | 'insufficient';
  resume_proposal: { intent_id: string; resume_payload: ResumePayload; briefing?: string | null } | null;
  tool_calls_made: string[];
  conversation_id?: string | null;
  cached_summary?: string | null;
}

export interface DashboardData {
  digest: Digest;
  intents: Intent[];
  current: CurrentIntent | null;
}

export interface RestoreReview {
  label: string;
  summary: string;
  projectTag?: string | null;
  payload: ResumePayload;
}

export type CommandMode = 'search' | 'copilot' | 'restore';

export type LLMProvider = 'openai' | 'groq' | 'gemini' | 'bedrock';

export interface LLMSettings {
  provider: LLMProvider;
  model: string;
  copilot_enabled: boolean;
  semantic_cluster_enabled: boolean;
  semantic_content_consent: boolean;
  semantic_full_capture_consent: boolean;
  semantic_timeout_ms: number;
  api_key_configured: boolean;
  credentials_configured: boolean;
  groq_base_url: string;
  google_cloud_project: string;
  google_cloud_location: string;
  bedrock_region: string;
  bedrock_profile: string;
}

export interface LLMSettingsUpdate {
  provider: LLMProvider;
  api_key?: string;
  clear_api_key?: boolean;
  model?: string;
  enable_copilot: boolean;
  enable_semantic_cluster?: boolean;
  enable_semantic_content_consent?: boolean;
  enable_semantic_full_capture_consent?: boolean;
  semantic_timeout_ms?: number;
  groq_base_url?: string;
  google_cloud_project?: string;
  google_cloud_location?: string;
  bedrock_region?: string;
  bedrock_profile?: string;
}

export interface DetailedCaptureSettings {
  editor: { enabled: boolean; excluded_patterns: string[] };
  browser: { enabled: boolean; context_enabled: boolean };
  filesystem: { enabled: boolean };
  approved_workspaces: string[];
  retention: string;
  export_includes_details: boolean;
}

export interface DetailedCaptureSettingsUpdate {
  editor?: { enabled?: boolean };
  browser?: { enabled?: boolean; context_enabled?: boolean };
  filesystem?: { enabled?: boolean };
}

export interface FilesystemCaptureSettings {
  all_accessible: boolean;
}

export interface RetentionPolicy {
  metadata_days: number | null;
  detailed_days: number | null;
}

export interface CaptureStatus {
  capture_paused?: boolean;
  detailed_capture?: {
    event_counts?: Record<string, number>;
  };
}

export interface IntelligencePreviewSample {
  semantic_clustering_example: Record<string, unknown>;
  labeling_example: Record<string, unknown>;
  copilot_example: Record<string, unknown>;
  disclosure: string[];
  browser_detailed_enabled: boolean;
  browser_context_enabled: boolean;
}

export type CapturePreset = 'metadata' | 'actions' | 'actions_excerpts';

export type SettingsTab = 'capture' | 'intelligence' | 'storage';
