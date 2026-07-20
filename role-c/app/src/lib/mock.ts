import type { CopilotResponse, CurrentIntent, DashboardData, Intent, ResumeSelectResponse, SearchResult } from '../types';

const authPayload = {
  files: ['~/projects/taskflow-app/src/auth.tsx'],
  urls: ['https://developer.mozilla.org/en-US/docs/Web/HTTP/Authentication', 'https://stackoverflow.com/questions/tagged/json-web-tokens'],
  shell: { cwd: '~/projects/taskflow-app', last_cmd: 'npm test -- auth' },
};

const childIntent: Intent = {
  id: 'mock-auth-tests',
  parent_id: 'mock-login-feature',
  date: '2026-07-13',
  label: 'Debug npm command',
  summary: 'Investigated failing authentication tests after updating the login flow.',
  confidence: 0.77,
  start_ts: 1783935000,
  end_ts: 1783937700,
  depth: 1,
  tags: ['project:taskflow-app'],
  stats: { event_count: 11, duration_seconds: 2700, sources: { shell: 5, vscode: 6 } },
  insights: {
    editor: [{ file: '~/projects/taskflow-app/src/auth.tsx' }],
    browser: [],
    shell: [{ command_family: 'npm', count: 2, exit_code: 1 }],
  },
  todos: [],
  resume_payload: authPayload,
  children: [],
};

const rootIntent: Intent = {
  id: 'mock-login-feature',
  parent_id: null,
  date: '2026-07-13',
  label: 'Building Login Feature',
  summary: 'Implemented the login flow, researched JWT behavior, and debugged failing npm tests.',
  confidence: 0.86,
  start_ts: 1783931400,
  end_ts: 1783938600,
  depth: 0,
  tags: ['project:taskflow-app'],
  stats: { event_count: 28, duration_seconds: 7200, sources: { vscode: 14, firefox: 9, shell: 5 } },
  insights: {
    editor: [{ file: '~/projects/taskflow-app/src/auth.tsx' }],
    browser: [{ domain: 'developer.mozilla.org' }],
    shell: [{ command_family: 'npm', count: 2, exit_code: 1 }],
  },
  todos: [],
  resume_payload: authPayload,
  children: [childIntent],
};

const researchIntent: Intent = {
  id: 'mock-token-research',
  parent_id: null,
  date: '2026-07-13',
  label: 'Research token refresh',
  summary: 'Compared token refresh approaches and browser security guidance.',
  confidence: 0.68,
  start_ts: 1783939200,
  end_ts: 1783940580,
  depth: 0,
  tags: ['project:taskflow-app'],
  stats: { event_count: 8, duration_seconds: 1380, sources: { firefox: 8 } },
  insights: { editor: [], browser: [{ domain: 'stackoverflow.com' }], shell: [] },
  todos: [],
  resume_payload: { files: [], urls: ['https://developer.mozilla.org/en-US/docs/Web/Security'], shell: {} },
  children: [],
};

const current: CurrentIntent = {
  label: 'Building Login Feature',
  summary: 'Working in auth.tsx',
  confidence: 0.78,
  since_ts: 1783931400,
};

export const mockDashboard: DashboardData = {
  digest: {
    date: '2026-07-13',
    headline: 'Building Login Feature',
    summary: 'Implemented login work, researched JWT behavior, and debugged npm tests.',
    top_intent_ids: [rootIntent.id],
    intent_count: 3,
    total_duration_seconds: 8580,
  },
  intents: [rootIntent, researchIntent],
  current,
};

function allIntents(intents: Intent[]): Intent[] {
  return intents.flatMap((intent) => [intent, ...allIntents(intent.children)]);
}

export function mockSearch(query: string): SearchResult[] {
  const normalized = query.toLowerCase();
  return allIntents(mockDashboard.intents)
    .filter((intent) => `${intent.label} ${intent.summary}`.toLowerCase().includes(normalized))
    .map((intent) => ({
      id: intent.id,
      label: intent.label,
      summary: intent.summary,
      date: intent.date,
      highlight_snippet: intent.summary,
    }));
}

export function mockResumeSelect(intentId?: string, query?: string): ResumeSelectResponse {
  const match = allIntents(mockDashboard.intents).find((intent) => intent.id === intentId)
    ?? mockSearch(query ?? '')
      .map((result) => allIntents(mockDashboard.intents).find((intent) => intent.id === result.id))
      .find((intent): intent is Intent => Boolean(intent));
  if (!match) return { needs_picker: false, candidates: [], selected: null };
  return {
    needs_picker: false,
    candidates: [],
    selected: {
      intent_id: match.id,
      label: match.label,
      summary: match.summary,
      project_tag: match.tags.find((tag) => tag.startsWith('project:'))?.slice(8) ?? null,
      workspace_root: match.resume_payload.shell.cwd ?? null,
      score: match.confidence,
      resume_payload: match.resume_payload,
    },
  };
}

export function mockCopilot(question: string): CopilotResponse {
  return {
    answer: `Stored activity indicates: ${mockDashboard.digest.summary} Your question was: “${question}”.`,
    citations: [{
      intent_id: rootIntent.id,
      date: rootIntent.date,
      label: rootIntent.label,
      summary: rootIntent.summary,
    }],
    confidence: 0.82,
    evidence_status: 'sufficient',
    resume_proposal: { intent_id: rootIntent.id, resume_payload: rootIntent.resume_payload, briefing: rootIntent.summary },
    tool_calls_made: ['search_intents'],
    conversation_id: 'mock-conversation',
    cached_summary: null,
  };
}
