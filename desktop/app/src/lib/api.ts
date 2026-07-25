import type {
  CopilotResponse,
  CurrentIntent,
  DashboardData,
  Digest,
  Intent,
  LLMSettings,
  LLMSettingsUpdate,
  RestoreResult,
  ResumeSelectResponse,
  SearchResult,
} from '../types';

const roleA = import.meta.env.VITE_ROLE_A_URL ?? 'http://127.0.0.1:9477';
const roleB = import.meta.env.VITE_ENGINE_URL ?? 'http://127.0.0.1:9478';

export class ApiError extends Error {
  constructor(
    public readonly status: number,
    message: string,
    public readonly payload?: unknown,
  ) {
    super(message);
    this.name = 'ApiError';
  }
}

function selectedDate(): string | undefined {
  const params = new URLSearchParams(window.location.search);
  return import.meta.env.VITE_INTENT_DATE || params.get('intentDate') || undefined;
}

async function request<T>(url: string, init?: RequestInit): Promise<T> {
  let status = 0;
  let ok = false;
  let raw = '';
  try {
    if (window.intent) {
      const response = await window.intent.request(url, init);
      status = response.status;
      ok = response.ok;
      raw = response.body;
    } else {
      const response = await fetch(url, init);
      status = response.status;
      ok = response.ok;
      raw = await response.text();
    }
  } catch {
    throw new ApiError(0, 'The local Intent service is unavailable.');
  }

  let payload: unknown = null;
  if (raw) {
    try {
      payload = JSON.parse(raw);
    } catch {
      payload = raw;
    }
  }
  if (!ok) {
    const detail = typeof payload === 'object' && payload !== null
      ? ('detail' in payload && typeof payload.detail === 'string' ? payload.detail : 'message' in payload && typeof payload.message === 'string' ? payload.message : undefined)
      : undefined;
    throw new ApiError(status, detail ?? `Local service request failed (${status}).`, payload);
  }
  return payload as T;
}

function queryString(path: string, parameters: Record<string, string | number | undefined>): string {
  const query = new URLSearchParams();
  for (const [key, value] of Object.entries(parameters)) {
    if (value !== undefined && value !== '') query.set(key, String(value));
  }
  const encoded = query.toString();
  return encoded ? `${path}?${encoded}` : path;
}

export const roleBApi = {
  async dashboard(): Promise<DashboardData> {
    const date = selectedDate();
    const [digest, intents, current] = await Promise.all([
      request<Digest>(queryString(`${roleB}/intents/digest`, { date })),
      request<Intent[]>(date ? queryString(`${roleB}/intents`, { date }) : `${roleB}/intents/yesterday`),
      request<CurrentIntent | null>(`${roleB}/intents/current`),
    ]);
    return { digest, intents, current };
  },

  async current(): Promise<CurrentIntent | null> {
    return request<CurrentIntent | null>(`${roleB}/intents/current`);
  },

  async search(query: string): Promise<SearchResult[]> {
    return request<SearchResult[]>(queryString(`${roleB}/intents/search`, { q: query, limit: 20 }));
  },

  async selectResume(selector: { intentId?: string; query?: string }): Promise<ResumeSelectResponse> {
    return request<ResumeSelectResponse>(`${roleB}/resume/select`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        ...(selector.intentId ? { intent_id: selector.intentId } : {}),
        ...(selector.query ? { query: selector.query } : {}),
        restore_scope: 'same_project',
      }),
    });
  },

  async askCopilot(question: string): Promise<CopilotResponse> {
    return request<CopilotResponse>(`${roleB}/copilot/query`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ question, mode: 'qa' }),
    });
  },

  async llmSettings(): Promise<LLMSettings> {
    return request<LLMSettings>(`${roleB}/settings/llm`);
  },

  async saveLlmSettings(settings: LLMSettingsUpdate): Promise<LLMSettings> {
    return request<LLMSettings>(`${roleB}/settings/llm`, {
      method: 'PUT',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(settings),
    });
  },

  async uploadGeminiCredentials(credentials: string): Promise<LLMSettings> {
    return request<LLMSettings>(`${roleB}/settings/llm/gemini-credentials`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ credentials }),
    });
  },

  async clearGeminiCredentials(): Promise<LLMSettings> {
    return request<LLMSettings>(`${roleB}/settings/llm/gemini-credentials`, {
      method: 'DELETE',
    });
  },
};

export const roleAApi = {
  async restore(payload: { mode: 'resume' | 'continue'; files: string[]; urls: string[]; shell: Record<string, string | undefined> }): Promise<RestoreResult> {
    return request<RestoreResult>(`${roleA}/v1/restore`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(payload),
    });
  },
};
