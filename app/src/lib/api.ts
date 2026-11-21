// Every daemon call goes through the Rust `api` command (reqwest + bearer
// token, which never reaches JS). With VITE_MOCK=1 an in-memory router answers.

import { command } from './bridge';
import { MOCK } from './env';
import type {
  BlockedDomainsConfig,
  CaptureStatus,
  CopilotQueryRequest,
  CopilotQueryResponse,
  CurrentIntent,
  DetailedCaptureConfig,
  DetailedCaptureUpdate,
  Digest,
  ForgetResponse,
  HealthResponse,
  Intent,
  IntentContextResponse,
  LlmSettingsSummary,
  LlmSettingsUpdate,
  RestoreRequest,
  RestoreResult,
  ResumeSelectRequest,
  ResumeSelectResponse,
  RetentionPolicy,
  RetentionPreview,
  SearchHit,
  WorkspacesConfig,
} from './types';

export const TOKEN_REFUSED = 'Intent refused the app’s access token. Restart Intent.';

export type Method = 'GET' | 'POST' | 'PUT' | 'DELETE';

export interface ApiResponse {
  status: number;
  body: unknown;
}

export class ApiError extends Error {
  readonly status: number;
  readonly payload: unknown;

  constructor(status: number, message: string, payload?: unknown) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.payload = payload;
  }

  /** The daemon could not be reached at all. */
  get unreachable(): boolean {
    return this.status === 0;
  }
}

function detailOf(payload: unknown): string | undefined {
  if (payload && typeof payload === 'object') {
    const record = payload as Record<string, unknown>;
    if (typeof record.detail === 'string') return record.detail;
    if (typeof record.message === 'string') return record.message;
    if (typeof record.error === 'string') return record.error;
  }
  return undefined;
}

async function transport(method: Method, path: string, body?: unknown): Promise<ApiResponse> {
  if (MOCK) {
    const { mockRequest } = await import('./mock');
    // The mock throws like a dead daemon; report it the same way as the Rust side.
    return mockRequest(method, path, body).catch((reason: unknown) => {
      throw new ApiError(0, reason instanceof Error ? reason.message : 'Intent isn’t running on this computer.');
    });
  }
  try {
    return await command<ApiResponse>('api', { method, path, body: body ?? null });
  } catch (reason) {
    const message = typeof reason === 'string' ? reason : reason instanceof Error ? reason.message : '';
    throw new ApiError(0, message || 'Intent isn’t running on this computer.');
  }
}

export async function request<T>(method: Method, path: string, body?: unknown): Promise<T> {
  const response = await transport(method, path, body);
  if (response.status < 200 || response.status >= 300) {
    // The Rust side retried a 401 with a freshly read token already. The
    // daemon's own text ("missing or invalid token") does not say what to do.
    const message =
      response.status === 401
        ? TOKEN_REFUSED
        : (detailOf(response.body) ?? `Request failed (${response.status}).`);
    throw new ApiError(response.status, message, response.body);
  }
  return response.body as T;
}

function qs(path: string, params: Record<string, string | number | null | undefined>): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined && value !== null && value !== '') search.set(key, String(value));
  }
  const encoded = search.toString();
  return encoded ? `${path}?${encoded}` : path;
}

export const api = {
  health: () => request<HealthResponse>('GET', '/healthz'),

  // Intents
  intentsFor: (date: string) => request<Intent[]>('GET', qs('/intents', { date })),
  yesterday: () => request<Intent[]>('GET', '/intents/yesterday'),
  intent: (id: string) => request<Intent>('GET', `/intents/${encodeURIComponent(id)}`),
  context: (id: string) => request<IntentContextResponse>('GET', `/intents/${encodeURIComponent(id)}/context`),
  digest: (date: string) => request<Digest>('GET', qs('/intents/digest', { date })),
  current: () => request<CurrentIntent | null>('GET', '/intents/current'),
  search: (q: string, opts: { limit?: number; date_from?: string; date_to?: string; project?: string } = {}) =>
    request<SearchHit[]>('GET', qs('/intents/search', { q, limit: opts.limit ?? 12, ...opts })),
  resumeSelect: (req: ResumeSelectRequest) => request<ResumeSelectResponse>('POST', '/resume/select', req),
  copilot: (req: CopilotQueryRequest) => request<CopilotQueryResponse>('POST', '/copilot/query', req),
  runPipeline: (date: string) => request<unknown>('POST', qs('/pipeline/run', { date })),
  forgetDate: (date: string) => request<ForgetResponse>('DELETE', `/v1/memory/date/${encodeURIComponent(date)}`),
  forgetProject: (project: string) =>
    request<ForgetResponse>('DELETE', `/v1/memory/project/${encodeURIComponent(project)}`),

  // LLM settings
  llmSettings: () => request<LlmSettingsSummary>('GET', '/settings/llm'),
  saveLlmSettings: (update: LlmSettingsUpdate) => request<LlmSettingsSummary>('PUT', '/settings/llm', update),

  // Capture
  status: () => request<CaptureStatus>('GET', '/v1/status'),
  pause: (paused: boolean, until?: number | null) =>
    request<{ ok: boolean; paused: boolean; until?: number | null }>('POST', '/v1/capture/pause', {
      paused,
      until: until ?? null,
    }),
  restore: (req: RestoreRequest) => request<RestoreResult>('POST', '/v1/restore', req),
  captureConfig: () => request<DetailedCaptureConfig>('GET', '/v1/detailed-capture/config'),
  saveCaptureConfig: (update: DetailedCaptureUpdate) =>
    request<DetailedCaptureConfig>('PUT', '/v1/detailed-capture/config', update),
  workspaces: () => request<WorkspacesConfig>('GET', '/v1/workspaces'),
  saveWorkspaces: (workspaces: string[]) => request<WorkspacesConfig>('PUT', '/v1/workspaces', { workspaces }),
  filesystemCapture: () => request<{ all_accessible: boolean }>('GET', '/v1/filesystem-capture'),
  saveFilesystemCapture: (all_accessible: boolean) =>
    request<{ all_accessible: boolean }>('PUT', '/v1/filesystem-capture', { all_accessible }),
  retentionPolicy: () => request<RetentionPolicy>('GET', '/v1/retention/policy'),
  saveRetentionPolicy: (policy: RetentionPolicy) => request<RetentionPolicy>('PUT', '/v1/retention/policy', policy),
  retentionPreview: (policy: RetentionPolicy) =>
    request<RetentionPreview>('GET', qs('/v1/retention/preview', { ...policy })),
  purgeRetention: (policy: RetentionPolicy) =>
    request<{ ok: boolean }>('POST', '/v1/retention/purge', { ...policy, confirm: true }),
  purgeDetailed: () => request<{ ok: boolean; deleted: number }>('POST', '/v1/detailed-capture/purge'),
  blockedDomains: () => request<BlockedDomainsConfig>('GET', '/v1/config'),
};
