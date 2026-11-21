// In-memory stand-in for intentd used with VITE_MOCK=1. Fixtures describe one
// believable developer's last two weeks. "Now" is frozen (see env.ts) so the
// screenshots are reproducible. URL flags: ?paused=1 starts paused,
// ?firstrun=1 shows onboarding, ?offline=1 simulates an unreachable daemon,
// ?denied=1 a daemon that refuses the app's token.

import type { ApiResponse, Method } from './api';
import { nowSec } from './env';
import { shiftIso, todayIso } from './format';
import type {
  CaptureStatus,
  CitedIntent,
  CopilotQueryRequest,
  CopilotQueryResponse,
  DetailedCaptureConfig,
  Digest,
  Intent,
  InsightRow,
  LlmSettingsSummary,
  LlmSettingsUpdate,
  RetentionPolicy,
  SearchHit,
} from './types';

const params = new URLSearchParams(typeof window !== 'undefined' ? window.location.search : '');
const HOME = '/home/pranav';

// ---------------------------------------------------------------------------
// Fixture builder
// ---------------------------------------------------------------------------

interface Spec {
  id: string;
  date: string;
  label: string;
  summary: string;
  start: string;
  end: string;
  project?: string;
  active?: number; // minutes
  files?: string[];
  lines?: number[];
  urls?: Array<[string, string]>; // [url, title]
  cwd?: string;
  cmd?: string;
  exit?: number;
  sources?: Record<string, number>;
  todos?: Array<[string, string]>;
  children?: Spec[];
  confidence?: number;
}

function at(date: string, hm: string): number {
  const [h, m] = hm.split(':').map(Number);
  const [y, mo, d] = date.split('-').map(Number);
  return Math.floor(new Date(y ?? 2026, (mo ?? 1) - 1, d ?? 1, h ?? 0, m ?? 0).getTime() / 1000);
}

function build(spec: Spec, parent: string | null = null, depth = 0): Intent {
  const start = at(spec.date, spec.start);
  const end = at(spec.date, spec.end);
  const files = (spec.files ?? []).map((f) => (f.startsWith('/') ? f : `${HOME}/${f}`));
  const editor: InsightRow[] = files.map((file, i) => ({ file, line: spec.lines?.[i], count: 40 - i * 9 }));
  const browser: InsightRow[] = (spec.urls ?? []).map(([url, title], i) => ({
    url,
    title,
    domain: new URL(url).hostname.replace(/^www\./, ''),
    count: 24 - i * 4,
  }));
  const shell: InsightRow[] = spec.cmd
    ? [{ command: spec.cmd, command_family: spec.cmd.split(' ')[0], count: 7, exit_code: spec.exit ?? 0 }]
    : [];
  const sources = spec.sources ?? {
    ...(files.length ? { vscode: 120 + files.length * 31 } : {}),
    ...(spec.urls?.length ? { firefox: 40 + spec.urls.length * 12 } : {}),
    ...(spec.cmd ? { shell: 18 } : {}),
    linux: 26,
  };
  const intent: Intent = {
    id: spec.id,
    parent_id: parent,
    date: spec.date,
    label: spec.label,
    summary: spec.summary,
    confidence: spec.confidence ?? 0.82,
    start_ts: start,
    end_ts: end,
    depth,
    tags: spec.project ? [`project:${spec.project}`] : [],
    stats: {
      event_count: Object.values(sources).reduce((a, b) => a + b, 0),
      duration_seconds: (spec.active ?? Math.round((end - start) / 60)) * 60,
      sources,
      unique_apps: [
        ...(files.length ? ['code'] : []),
        ...(spec.urls?.length ? ['firefox'] : []),
        ...(spec.cmd ? ['gnome-terminal'] : []),
      ],
    },
    insights: { editor, browser, shell },
    todos: (spec.todos ?? []).map(([path, marker], i) => ({
      path: `${HOME}/${path}`,
      marker,
      observed_ts: start + 600 * (i + 1),
    })),
    evidence: [],
    resume_payload: {
      files,
      urls: (spec.urls ?? []).map(([url]) => url),
      shell: spec.cwd || spec.cmd ? { cwd: spec.cwd ? `${HOME}/${spec.cwd}` : null, last_cmd: spec.cmd ?? null } : {},
    },
    prefix: null,
    semantic: null,
    privacy_policy_version: 'safe-intent-features-v2',
    children: [],
  };
  intent.children = (spec.children ?? []).map((c) => build({ ...c, date: spec.date, project: spec.project }, spec.id, depth + 1));
  return intent;
}

const TODAY = todayIso();
const D1 = shiftIso(TODAY, -1);
const D2 = shiftIso(TODAY, -2);

const specs: Spec[] = [
  {
    id: 'int_7f3a01',
    date: TODAY,
    label: 'Triage overnight payment alerts',
    summary: 'Checked the payments pods after two latency alerts; both cleared on their own after a node drain.',
    start: '08:30',
    end: '08:56',
    project: 'ops',
    active: 22,
    urls: [
      ['https://grafana.acme.dev/d/payments/latency?from=now-12h', 'Payments latency · Grafana'],
      ['https://acme.pagerduty.com/incidents/Q2X81', 'Incident Q2X81 · PagerDuty'],
    ],
    cwd: 'work/infra',
    cmd: 'kubectl -n payments get pods -o wide',
  },
  {
    id: 'int_7f3a02',
    date: TODAY,
    label: 'Port retention purge into intentd',
    summary:
      'Moved the retention purge from the Python capture server into intent-store with a single write transaction, and added cutoff tests for the detailed and metadata tiers.',
    start: '09:12',
    end: '11:04',
    project: 'intent',
    active: 101,
    files: [
      'code/intent/crates/intentd/src/retention.rs',
      'code/intent/crates/intent-store/src/purge.rs',
      'code/intent/crates/intent-types/src/lib.rs',
    ],
    lines: [142, 58, 611],
    urls: [
      ['https://docs.rs/rusqlite/latest/rusqlite/struct.Transaction.html', 'Transaction in rusqlite - Rust'],
      ['https://www.sqlite.org/lang_delete.html', 'DELETE · SQLite'],
      ['https://github.com/Pranav5255/Intent/issues/57', 'Retention purge leaves orphaned FTS rows · Issue #57'],
    ],
    cwd: 'code/intent',
    cmd: 'cargo test -p intent-store purge -- --nocapture',
    todos: [
      ['code/intent/crates/intent-store/src/purge.rs', 'TODO'],
      ['code/intent/crates/intentd/src/retention.rs', 'FIXME'],
    ],
    children: [
      {
        id: 'int_7f3a02a',
        date: TODAY,
        label: 'Write cutoff tests for purge tiers',
        summary: 'Golden tests for 7/30/90-day cutoffs.',
        start: '10:02',
        end: '10:48',
        files: ['code/intent/crates/intent-store/tests/purge.rs'],
        lines: [33],
        cwd: 'code/intent',
        cmd: 'cargo test -p intent-store purge',
      },
      {
        id: 'int_7f3a02b',
        date: TODAY,
        label: 'Read rusqlite transaction docs',
        summary: 'Checked savepoint semantics before batching deletes.',
        start: '09:20',
        end: '09:34',
        urls: [['https://docs.rs/rusqlite/latest/rusqlite/struct.Transaction.html', 'Transaction in rusqlite - Rust']],
      },
    ],
  },
  {
    id: 'int_7f3a03',
    date: TODAY,
    label: 'Review billing webhook PR #482',
    summary:
      'Reviewed the Stripe webhook idempotency change; asked for the replay window to be configurable and for a test covering duplicate deliveries.',
    start: '11:20',
    end: '12:05',
    project: 'ledger-api',
    active: 41,
    files: ['work/ledger-api/src/webhooks/stripe.ts', 'work/ledger-api/src/webhooks/idempotency.ts'],
    lines: [88, 21],
    urls: [
      ['https://github.com/acme/ledger-api/pull/482/files', 'Make webhook handling idempotent by mkoval · Pull Request #482'],
      ['https://docs.stripe.com/webhooks/signature', 'Check the webhook signatures · Stripe Docs'],
    ],
    cwd: 'work/ledger-api',
    cmd: 'pnpm test webhooks',
  },
  {
    id: 'int_7f3a04',
    date: TODAY,
    label: 'Fix flaky checkout e2e test',
    summary:
      'Reproduced the checkout flake about one run in eight. The cart badge renders before /api/cart resolves; waiting on that response made 20 repeated runs pass.',
    start: '13:10',
    end: '14:25',
    project: 'storefront',
    active: 64,
    files: ['work/storefront/tests/e2e/checkout.spec.ts', 'work/storefront/src/cart/useCart.ts'],
    lines: [57, 112],
    urls: [
      ['https://playwright.dev/docs/test-retries', 'Retries | Playwright'],
      ['https://github.com/acme/storefront/actions/runs/1183920', 'e2e · acme/storefront · Run #1183920'],
      ['https://playwright.dev/docs/api/class-page#page-wait-for-response', 'Page | Playwright'],
    ],
    cwd: 'work/storefront',
    cmd: 'npx playwright test checkout --repeat-each 20',
    exit: 0,
    todos: [['work/storefront/src/cart/useCart.ts', 'TODO']],
    children: [
      {
        id: 'int_7f3a04a',
        date: TODAY,
        label: 'Reproduce flake locally',
        summary: 'Ran the spec 40 times with tracing on.',
        start: '13:10',
        end: '13:42',
        files: ['work/storefront/tests/e2e/checkout.spec.ts'],
        lines: [57],
        cwd: 'work/storefront',
        cmd: 'npx playwright test checkout --repeat-each 40 --trace on',
        exit: 1,
      },
      {
        id: 'int_7f3a04b',
        date: TODAY,
        label: 'Wait for /api/cart before asserting badge',
        summary: 'Added waitForResponse and removed the fixed timeout.',
        start: '13:50',
        end: '14:25',
        files: ['work/storefront/tests/e2e/checkout.spec.ts', 'work/storefront/src/cart/useCart.ts'],
        lines: [57, 112],
      },
    ],
  },
  {
    id: 'int_7f3a05',
    date: TODAY,
    label: 'Draft Q4 infra cost memo',
    summary: 'Pulling October spend by service into the memo; compute is up 18% from the new search cluster.',
    start: '14:40',
    end: '15:38',
    project: 'docs',
    active: 49,
    urls: [
      ['https://docs.google.com/document/d/1qX4-infra-q4/edit', 'Q4 infrastructure cost review · Google Docs'],
      ['https://us-east-1.console.aws.amazon.com/costmanagement/home#/cost-explorer', 'Cost Explorer · AWS'],
      ['https://grafana.acme.dev/d/search-cluster/overview', 'Search cluster · Grafana'],
      ['https://www.notion.so/acme/Infra-budget-2026-8f1c', 'Infra budget 2026 · Notion'],
    ],
  },
  // Yesterday
  {
    id: 'int_6e2b01',
    date: D1,
    label: 'Design bearer-token auth for intentd',
    summary: 'Settled on a 0600 token file under XDG_RUNTIME_DIR; mutating routes require it, ingest stays open.',
    start: '09:40',
    end: '12:10',
    project: 'intent',
    active: 128,
    files: ['code/intent/docs/REWRITE.md', 'code/intent/crates/intentd/src/auth.rs'],
    lines: [24, 40],
    urls: [['https://docs.rs/axum/latest/axum/middleware/index.html', 'axum::middleware - Rust']],
    cwd: 'code/intent',
    cmd: 'cargo clippy -p intentd -- -D warnings',
  },
  {
    id: 'int_6e2b02',
    date: D1,
    label: 'Hybrid search ranking experiments',
    summary: 'Compared FTS5 bm25 alone against bm25 plus sqlite-vec cosine; the blend fixed paraphrased queries.',
    start: '13:30',
    end: '16:05',
    project: 'intent',
    active: 131,
    files: ['code/intent/crates/intent-store/src/search.rs'],
    lines: [77],
    urls: [
      ['https://alexgarcia.xyz/sqlite-vec/features/knn.html', 'KNN queries · sqlite-vec'],
      ['https://www.sqlite.org/fts5.html', 'SQLite FTS5 Extension'],
    ],
    cwd: 'code/intent',
    cmd: 'cargo bench -p intent-store search',
  },
  {
    id: 'int_6e2b03',
    date: D1,
    label: 'Update storefront onboarding docs',
    summary: 'Rewrote the local setup section for the new pnpm workspace.',
    start: '16:30',
    end: '17:05',
    project: 'storefront',
    files: ['work/storefront/README.md'],
    lines: [12],
  },
  // Two days ago
  {
    id: 'int_5d1c01',
    date: D2,
    label: 'Migrate CI to cached runners',
    summary: 'Moved the storefront pipeline to cached runners; cold builds went from 14m to 6m.',
    start: '09:30',
    end: '11:00',
    project: 'infra',
    files: ['work/storefront/.github/workflows/ci.yml'],
    lines: [41],
    urls: [['https://docs.github.com/en/actions/using-jobs/choosing-the-runner-for-a-job', 'Choosing the runner for a job']],
    cwd: 'work/storefront',
    cmd: 'gh run watch',
  },
  {
    id: 'int_5d1c02',
    date: D2,
    label: 'First look at checkout flake',
    summary: 'Saw the flake in CI twice; no local repro yet.',
    start: '13:00',
    end: '13:40',
    project: 'storefront',
    urls: [['https://github.com/acme/storefront/actions/runs/1181002', 'e2e · acme/storefront · Run #1181002']],
  },
  {
    id: 'int_5d1c03',
    date: D2,
    label: 'Write ADR: one daemon instead of five',
    summary: 'Argued for a single Rust daemon to replace the capture server, engine, tracker and tray.',
    start: '15:00',
    end: '16:20',
    project: 'intent',
    files: ['code/intent/docs/adr/0007-single-daemon.md'],
    lines: [1],
  },
];

// Older days: deterministic filler so the 14-day timeline has texture.
const POOL: Array<[string, string, string]> = [
  ['intent', 'Refine resume selection scoring', 'Weighted recency over project match for resume candidates.'],
  ['storefront', 'Cart page performance pass', 'Trimmed hydration on the cart page.'],
  ['ledger-api', 'Reconcile November invoices job', 'Fixed rounding in the reconciliation job.'],
  ['infra', 'Rotate staging database credentials', 'Rotated and redeployed staging secrets.'],
  ['docs', 'Write incident review for INC-311', 'Timeline and follow-ups for the cache outage.'],
  ['intent', 'GNOME Shell focus extension spike', 'Prototyped focused-window reporting over D-Bus.'],
];
for (let back = 3; back <= 13; back += 1) {
  const date = shiftIso(TODAY, -back);
  const weekday = new Date(`${date}T12:00:00`).getDay();
  const n = weekday === 0 || weekday === 6 ? 1 : 2 + (back % 2);
  for (let i = 0; i < n; i += 1) {
    const [project, label, summary] = POOL[(back * 3 + i) % POOL.length] as [string, string, string];
    const startH = 9 + i * 3 + (back % 2);
    specs.push({
      id: `int_old_${back}_${i}`,
      date,
      label,
      summary,
      start: `${String(startH).padStart(2, '0')}:${back % 2 ? '15' : '40'}`,
      end: `${String(startH + 1 + (i % 2)).padStart(2, '0')}:${back % 3 ? '05' : '30'}`,
      project,
      files: project === 'docs' ? [] : [`code/${project}/src/lib.rs`],
      urls: [['https://github.com/acme/' + project, `acme/${project}`]],
    });
  }
}

const INTENTS: Intent[] = specs.map((s) => build(s));

function rootsFor(date: string): Intent[] {
  return INTENTS.filter((i) => i.date === date).sort((a, b) => a.start_ts - b.start_ts);
}

function allNodes(): Intent[] {
  const out: Intent[] = [];
  const walk = (i: Intent) => {
    out.push(i);
    i.children.forEach(walk);
  };
  INTENTS.forEach(walk);
  return out;
}

function findIntent(id: string): Intent | undefined {
  return allNodes().find((i) => i.id === id);
}

// ---------------------------------------------------------------------------
// Mutable mock state
// ---------------------------------------------------------------------------

const state = {
  offline: params.get('offline') === '1',
  denied: params.get('denied') === '1',
  paused: params.get('paused') === '1',
  pausedUntil: params.get('paused') === '1' ? at(TODAY, '16:30') : (null as number | null),
  workspaces: [`${HOME}/code/intent`, `${HOME}/work/storefront`, `${HOME}/work/ledger-api`],
  retention: { metadata_days: 90, detailed_days: 30 } as RetentionPolicy,
  allAccessible: false,
  capture: {
    editor: { enabled: true, excluded_patterns: ['*.env', '*.pem', 'id_*'] },
    browser: { enabled: true, context_enabled: false },
    filesystem: { enabled: true },
  },
  llm: {
    provider: 'local',
    model: 'qwen3:8b',
    base_url: 'http://127.0.0.1:11434/v1',
    embedding_model: 'nomic-embed-text',
    copilot_enabled: true,
    semantic_cluster_enabled: true,
    semantic_content_consent: false,
    semantic_full_capture_consent: false,
    semantic_timeout_ms: 8000,
    api_key_configured: false,
    credentials_configured: false,
    google_cloud_project: null,
    google_cloud_location: null,
    bedrock_region: null,
    bedrock_profile: null,
    local_server_reachable: true,
  } as LlmSettingsSummary,
};

function detailedConfig(): DetailedCaptureConfig {
  const r = state.retention;
  return {
    ...state.capture,
    approved_workspaces: state.workspaces,
    retention:
      r.metadata_days === null && r.detailed_days === null
        ? 'indefinite'
        : `metadata ${r.metadata_days ?? '∞'}d, detailed ${r.detailed_days ?? '∞'}d`,
    export_includes_details: true,
  };
}

function captureStatus(): CaptureStatus {
  const now = nowSec();
  if (state.paused && state.pausedUntil && now >= state.pausedUntil) {
    state.paused = false;
    state.pausedUntil = null;
  }
  const src = (count: number, ago: number | null) => ({
    event_count: count,
    last_event_ts: ago === null ? null : now - ago,
    last_ingested_at: ago === null ? null : now - ago,
    last_ingest_lag_seconds: ago === null ? null : 0,
    healthy: ago !== null && ago < 1800,
  });
  return {
    ok: true,
    session_type: 'wayland',
    capture_paused: state.paused,
    capture_paused_until: state.paused ? state.pausedUntil : null,
    sources: {
      vscode: src(48_211, 41),
      firefox: src(31_904, 12),
      shell: src(6_877, 190),
      linux: src(92_310, 5),
      filesystem: src(14_066, 128),
    },
    services: { event_server: true, x11_tracker: true, workspace_watch: true },
    detailed_capture: {
      ...detailedConfig(),
      event_counts: { 'vscode/document_change': 9_412, 'firefox/user_action': 3_208, 'filesystem/file_content': 611 },
    },
    db_size_bytes: 187_430_912,
    config_warnings: [],
  };
}

// ---------------------------------------------------------------------------
// Search and Copilot
// ---------------------------------------------------------------------------

function haystack(i: Intent): string {
  return [
    i.label,
    i.summary,
    ...i.tags,
    ...i.resume_payload.files,
    ...i.resume_payload.urls,
    i.resume_payload.shell.last_cmd ?? '',
    ...i.insights.browser.map((b) => String(b.title ?? '')),
  ]
    .join(' ')
    .toLowerCase();
}

function highlight(text: string, terms: string[]): string {
  let out = text;
  for (const term of terms) {
    if (term.length < 2) continue;
    out = out.replace(new RegExp(`(${term.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')})`, 'ig'), '**$1**');
  }
  return out;
}

function search(q: string, dateFrom?: string | null, limit = 12): SearchHit[] {
  const terms = q.toLowerCase().split(/\s+/).filter(Boolean);
  // Light synonym table imitates the semantic half of hybrid search.
  const synonyms: Record<string, string[]> = {
    flaky: ['flake'],
    tests: ['test'],
    delete: ['purge', 'retention'],
    costs: ['cost', 'spend'],
    auth: ['token', 'bearer'],
    payments: ['stripe', 'billing', 'payment'],
  };
  const hits: SearchHit[] = [];
  for (const intent of allNodes()) {
    if (dateFrom && intent.date < dateFrom) continue;
    const hay = haystack(intent);
    let score = 0;
    for (const term of terms) {
      if (hay.includes(term)) score += 1;
      else if ((synonyms[term] ?? []).some((s) => hay.includes(s))) score += 0.6;
      else score -= 0.5;
    }
    if (score <= 0) continue;
    const recency = Math.max(0, 1 - (nowSec() - intent.end_ts) / (14 * 86_400));
    hits.push({
      id: intent.id,
      label: intent.label,
      summary: intent.summary,
      date: intent.date,
      highlight_snippet: highlight(intent.summary, terms),
      score: score / terms.length + recency * 0.3 - intent.depth * 0.05,
    });
  }
  return hits.sort((a, b) => b.score - a.score).slice(0, limit);
}

function cite(id: string): CitedIntent {
  const i = findIntent(id);
  return { intent_id: id, date: i?.date ?? TODAY, label: i?.label ?? '', summary: i?.summary ?? '' };
}

function copilot(req: CopilotQueryRequest): CopilotQueryResponse {
  const q = req.question.toLowerCase();
  if (/checkout|flak|e2e|playwright/.test(q)) {
    return {
      answer:
        'The checkout flake came from the cart badge rendering before /api/cart resolved. You first saw it in CI two days ago [2] and reproduced it today at roughly one run in eight [1]. Waiting on the /api/cart response instead of a fixed timeout made 20 repeated runs pass; the change in useCart.ts still has a TODO to remove the old retry.',
      citations: [cite('int_7f3a04'), cite('int_5d1c02')],
      confidence: 0.86,
      evidence_status: 'sufficient',
      resume_proposal: { intent_id: 'int_7f3a04', resume_payload: findIntent('int_7f3a04')!.resume_payload, briefing: null },
      tool_calls_made: ['search_intents', 'get_intent'],
      conversation_id: 'conv_mock_1',
      cached_summary: null,
    };
  }
  if (/cost|memo|spend|infra/.test(q)) {
    return {
      answer:
        'You started the Q4 infra cost memo at 14:40 and are still on it [1]. So far it pulls October spend by service from Cost Explorer; the open question is the 18% compute increase from the new search cluster.',
      citations: [cite('int_7f3a05')],
      confidence: 0.74,
      evidence_status: 'sufficient',
      resume_proposal: null,
      tool_calls_made: ['get_current_intent'],
      conversation_id: 'conv_mock_2',
      cached_summary: null,
    };
  }
  return {
    answer:
      'This morning you moved the retention purge out of the old capture server and into intent-store, wrapping both tiers in one write transaction [1]. Cutoff tests for 7, 30 and 90 days pass [2]. Two markers are still open: a TODO in purge.rs about orphaned search rows (issue #57) and a FIXME in retention.rs:142. Yesterday’s auth design [3] is what the purge route now sits behind.',
    citations: [cite('int_7f3a02'), cite('int_7f3a02a'), cite('int_6e2b01')],
    confidence: 0.88,
    evidence_status: 'sufficient',
    resume_proposal: { intent_id: 'int_7f3a02', resume_payload: findIntent('int_7f3a02')!.resume_payload, briefing: null },
    tool_calls_made: ['search_intents', 'get_intent', 'get_intent'],
    conversation_id: 'conv_mock_3',
    cached_summary: null,
  };
}

function digest(date: string): Digest {
  const roots = rootsFor(date);
  const total = roots.reduce((sum, i) => sum + i.stats.duration_seconds, 0);
  if (date === TODAY) {
    return {
      date,
      headline: 'Retention purge landed in intentd; checkout flake traced to a cart race',
      summary:
        'Most of the morning went into the Rust port of retention, which now deletes both tiers in one transaction and is covered by cutoff tests. After lunch you narrowed the storefront flake to the cart badge asserting before /api/cart returns, then moved on to the Q4 cost memo, which is still open.',
      top_intent_ids: ['int_7f3a02', 'int_7f3a04', 'int_7f3a05'],
      intent_count: roots.length,
      total_duration_seconds: total,
    };
  }
  return {
    date,
    headline: roots[0] ? `Mostly ${roots[0].label.toLowerCase()}` : 'No recorded work',
    summary: roots.map((r) => r.summary).slice(0, 2).join(' '),
    top_intent_ids: roots.slice(0, 3).map((r) => r.id),
    intent_count: roots.length,
    total_duration_seconds: total,
  };
}

function contextMarkdown(i: Intent): string {
  const lines = [
    `# ${i.label}`,
    '',
    `_${i.date} · ${i.tags.join(', ') || 'no project'}_`,
    '',
    i.summary,
    '',
    '## Files',
    ...i.resume_payload.files.map((f) => `- \`${f}\``),
    '',
    '## Tabs',
    ...i.resume_payload.urls.map((u) => `- ${u}`),
  ];
  if (i.resume_payload.shell.cwd) {
    lines.push('', '## Terminal', `- cwd: \`${i.resume_payload.shell.cwd}\``);
    if (i.resume_payload.shell.last_cmd) lines.push(`- last command: \`${i.resume_payload.shell.last_cmd}\``);
  }
  return lines.join('\n');
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

const ok = (body: unknown, status = 200): ApiResponse => ({ status, body });
const notFound = (detail = 'Not found'): ApiResponse => ({ status: 404, body: { detail } });
const delay = (ms: number) => new Promise((r) => setTimeout(r, ms));

export async function mockRequest(method: Method, rawPath: string, body: unknown): Promise<ApiResponse> {
  if (state.offline) {
    await delay(120);
    throw new Error('Intent isn’t running on this computer.');
  }
  const url = new URL(rawPath, 'http://mock');
  const path = url.pathname;
  // The daemon's own 401 body; /healthz needs no token.
  if (state.denied && path !== '/healthz') return { status: 401, body: { error: 'missing or invalid token' } };
  const q = url.searchParams;
  await delay(path.startsWith('/copilot') ? 900 : path.startsWith('/v1/restore') ? 500 : 40);

  if (path === '/healthz') return ok({ ok: true, version: '2.0.0' });
  if (path === '/intents/yesterday') return ok(rootsFor(D1));
  if (path === '/intents' && method === 'GET') return ok(rootsFor(q.get('date') ?? TODAY));
  if (path === '/intents/digest') return ok(digest(q.get('date') ?? D1));
  if (path === '/intents/current') {
    const current = findIntent('int_7f3a05')!;
    return ok(
      state.paused ? null : { label: current.label, summary: current.summary, confidence: 0.78, since_ts: current.start_ts },
    );
  }
  if (path === '/intents/search') {
    return ok(search(q.get('q') ?? '', q.get('date_from'), Number(q.get('limit') ?? 12)));
  }
  if (path === '/intents/stats') return ok({ intent_count: INTENTS.length });
  const ctx = path.match(/^\/intents\/([^/]+)\/context$/);
  if (ctx) {
    const i = findIntent(decodeURIComponent(ctx[1]!));
    return i ? ok({ intent_id: i.id, markdown: contextMarkdown(i) }) : notFound('Intent not found');
  }
  const one = path.match(/^\/intents\/([^/]+)$/);
  if (one) {
    const i = findIntent(decodeURIComponent(one[1]!));
    return i ? ok(i) : notFound('Intent not found');
  }
  if (path === '/resume/select') {
    const id = (body as { intent_id?: string } | null)?.intent_id;
    const i = id ? findIntent(id) : INTENTS.find((x) => x.date === TODAY);
    if (!i) return notFound('No stored intent matched the selection');
    const project = i.tags.find((t) => t.startsWith('project:'))?.slice(8) ?? null;
    return ok({
      needs_picker: false,
      candidates: [],
      selected: {
        intent_id: i.id,
        label: i.label,
        summary: i.summary,
        project_tag: project,
        workspace_root: i.resume_payload.shell.cwd ?? null,
        score: 0.92,
        resume_payload: i.resume_payload,
      },
    });
  }
  if (path === '/copilot/query') return ok(copilot(body as CopilotQueryRequest));
  if (path === '/pipeline/run') {
    await delay(1200);
    return ok({ intents: rootsFor(q.get('date') ?? TODAY), warnings: [], source_hash: 'mock', pipeline_version: '2.0.0', cached: false });
  }
  const forgetDate = path.match(/^\/v1\/memory\/date\/(.+)$/);
  if (forgetDate && method === 'DELETE') {
    const date = decodeURIComponent(forgetDate[1]!);
    const ids = rootsFor(date).map((i) => i.id);
    return ok({ deleted_intent_ids: ids, deleted_count: ids.length, deleted_event_count: 412, affected_dates: [date] });
  }
  if (path.startsWith('/v1/memory/project/') && method === 'DELETE') {
    return ok({ deleted_intent_ids: [], deleted_count: 7, deleted_event_count: 1_204, affected_dates: [TODAY] });
  }

  if (path === '/settings/llm') {
    if (method === 'PUT') {
      const u = body as LlmSettingsUpdate;
      state.llm = {
        ...state.llm,
        provider: u.provider,
        model: u.model ?? state.llm.model,
        base_url: u.base_url ?? state.llm.base_url,
        embedding_model: u.embedding_model ?? state.llm.embedding_model,
        copilot_enabled: u.enable_copilot ?? state.llm.copilot_enabled,
        semantic_cluster_enabled: u.enable_semantic_cluster ?? state.llm.semantic_cluster_enabled,
        semantic_content_consent: u.enable_semantic_content_consent ?? state.llm.semantic_content_consent,
        api_key_configured: u.clear_api_key ? false : u.api_key ? true : state.llm.api_key_configured,
        google_cloud_project: u.google_cloud_project ?? state.llm.google_cloud_project,
        google_cloud_location: u.google_cloud_location ?? state.llm.google_cloud_location,
        bedrock_region: u.bedrock_region ?? state.llm.bedrock_region,
        bedrock_profile: u.bedrock_profile ?? state.llm.bedrock_profile,
        local_server_reachable: u.provider === 'local' ? !(u.base_url ?? '').includes('9999') : null,
      };
    }
    return ok(state.llm);
  }

  if (path === '/v1/status' || path === '/status') return ok(captureStatus());
  if (path === '/v1/capture/pause') {
    const b = body as { paused: boolean; until?: number | null };
    state.paused = b.paused;
    state.pausedUntil = b.paused ? (b.until ?? null) : null;
    return ok({ ok: true, paused: state.paused, until: state.pausedUntil });
  }
  if (path === '/v1/restore') {
    const b = body as { files: string[]; urls: string[]; shell: { cwd?: string } };
    return ok({ ok: true, restored: { files: b.files.length, urls: b.urls.length, shell: Boolean(b.shell.cwd) }, failed: [] });
  }
  if (path === '/v1/detailed-capture/config') {
    if (method === 'PUT') {
      const u = body as Partial<typeof state.capture>;
      state.capture = {
        editor: { ...state.capture.editor, ...(u.editor ?? {}) },
        browser: { ...state.capture.browser, ...(u.browser ?? {}) },
        filesystem: { ...state.capture.filesystem, ...(u.filesystem ?? {}) },
      };
    }
    return ok(detailedConfig());
  }
  if (path === '/v1/workspaces') {
    if (method === 'PUT') state.workspaces = (body as { workspaces: string[] }).workspaces;
    return ok({ workspaces: state.workspaces });
  }
  if (path === '/v1/filesystem-capture') {
    if (method === 'PUT') state.allAccessible = (body as { all_accessible: boolean }).all_accessible;
    return ok({ all_accessible: state.allAccessible });
  }
  if (path === '/v1/retention/policy') {
    if (method === 'PUT') state.retention = body as RetentionPolicy;
    return ok(state.retention);
  }
  if (path === '/v1/retention/preview') {
    const d = Number(q.get('detailed_days') ?? 0);
    const m = Number(q.get('metadata_days') ?? 0);
    const detailed = d ? Math.round(13_231 * Math.max(0, 1 - d / 60)) + 214 : 0;
    const metadata = m ? Math.round(193_368 * Math.max(0, 1 - m / 180)) + 1_020 : 0;
    const intents = m ? Math.round(1_840 * Math.max(0, 1 - m / 180)) + 12 : 0;
    return ok({ now: nowSec(), detailed_days: d || null, metadata_days: m || null, eligible: { detailed, metadata, total: detailed + metadata, intents } });
  }
  if (path === '/v1/retention/purge') return ok({ ok: true });
  if (path === '/v1/detailed-capture/purge') return ok({ ok: true, deleted: 13_231 });
  if (path === '/v1/config') {
    return ok({ blocked_domains: ['accounts.google.com', 'mail.google.com', '1password.com', '*.bank', 'chase.com', 'web.whatsapp.com'] });
  }
  return notFound(`Mock has no route for ${method} ${path}`);
}

export const MOCK_FIRST_RUN = params.get('firstrun') === '1';
