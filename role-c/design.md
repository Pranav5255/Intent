# Intent OS — Role C Frontend Design Spec

This document is the source of truth for building the **production Role C frontend**. It translates the validated preview in `role-c/preview/` into a shippable Ubuntu overlay app.

**Audience:** Codex / frontend implementers  
**Reference implementation:** `role-c/preview/index.html` (visual + interaction prototype)  
**Backend contracts:** `ROLE-C-HANDOFF.md`, Role B `intent_engine/api.py`, Role A `event_server/main.py`

---

## 1. Product summary

Intent OS Role C is a **non-intrusive desktop overlay** that helps users:

1. **Recall** what they were working on (yesterday’s sessions, digest, search).
2. **Resume** prior work with one click (files, URLs, terminal context via Role A restore).
3. **Ask** optional Copilot questions grounded in stored intents (Role B only).

Role C is **presentation + user-confirmed actions**. It must not infer intents, cluster events, or invent restore payloads.

### Demo narrative (golden fixture)

Locked date: **2026-07-13**  
Story: **Building Login Feature** in `~/projects/taskflow-app/src/auth.tsx` — npm test failures, MDN/Stack Overflow research, JWT debugging.

Seed data:

```bash
curl -s -X POST http://127.0.0.1:9478/pipeline/run-replay \
  -H "Content-Type: application/json" \
  --data-binary @role-b/tests/fixtures/demo-day.json
```

---

## 2. Platform & runtime

| Layer | Choice | Notes |
|---|---|---|
| Ship target | **Electron** on Ubuntu/X11 | Transparent always-on-top overlay |
| Preview (done) | Vanilla HTML in `role-c/preview/` | Browser-served; wallpaper simulates blur |
| Production UI | **Recommended:** React + TypeScript + Vite inside Electron | Port layout/CSS from preview; split into components |
| Voice (v1 preview) | Web Speech API | Production: optional Whisper/local STT later |
| Port (dev) | 9479 | Role B CORS already allows `localhost:9479` and `:5173` |

### Electron window requirements

```javascript
{
  transparent: true,
  frame: false,
  hasShadow: false,
  alwaysOnTop: 'screen-saver',
  skipTaskbar: false,
  resizable: false,
  fullscreenable: false,
  webPreferences: { preload: 'preload.js', contextIsolation: true }
}
```

**Mouse passthrough:** When overlay is idle (bar closed), call `setIgnoreMouseEvents(true, { forward: true })`. When bar/toast is open, `setIgnoreMouseEvents(false)`.

**Global shortcut:** `Ctrl+Space` toggles command bar (register in main process; do not rely on page focus alone).

**No wallpaper in production:** Electron window is transparent; blur comes from `backdrop-filter` over the real desktop.

---

## 3. The three UI surfaces

All three share the **liquid-glass** design system (Section 5). Only one surface should capture pointer events at a time.

```
┌─────────────────────────────────────────────────────────────┐
│                    [ Command Bar — pill ]                    │  ← top center, 6vh from top
│                         ↓ expands                            │
│              ┌─────────────────────────┐                     │
│              │   Session Dashboard      │                     │
│              │   (digest hero + cards)  │                     │
│              └─────────────────────────┘                     │
│                                                              │
│                                                              │
│                                    ┌──────────────────┐    │
│                                    │ Notification toast│    │  ← bottom-right
│                                    └──────────────────┘    │
└─────────────────────────────────────────────────────────────┘
```

### 3.1 Command bar (primary entry)

**Purpose:** Universal input — search, Copilot questions, voice, keyboard navigation.

| Property | Value |
|---|---|
| Position | Top center, `padding-top: 6vh` |
| Size | 640px × 56px (max-width `calc(100vw - 32px)`) |
| Shape | Pill, radius 22px |
| Contents | App glyph · text input · mic button · `Esc` hint |

**Input modes** (parse first character):

| Prefix | Mode | Backend |
|---|---|---|
| `/query` | Search | `GET /intents/search?q=` |
| `?question` | Copilot Q&A | `POST /copilot/query` `{ question, mode: "qa" }` |
| `!` | Restore top match | `POST /v1/restore` with selected intent payload |
| (default) | Free-text search | Same as `/` |

**Keyboard**

| Key | Action |
|---|---|
| `Ctrl+Space` | Toggle bar open/closed |
| `Esc` | Close bar + dashboard |
| `↑` / `↓` | Move selection across flat intent rows (parent + children) |
| `Enter` | Execute current mode / restore selected row |

**Voice:** Mic toggles Web Speech recognition; streams transcript into input; pulse animation while listening. Fail gracefully if unsupported.

**Open animation:** opacity 0→1, translateY -12px→0, scale 0.98→1 over 180ms spring.

### 3.2 Session dashboard (expanded panel)

**Purpose:** Show restorable sessions extending **downward from the command bar** (not a separate page).

| Property | Value |
|---|---|
| Attachment | Directly below bar, 10px gap |
| Max height | 62vh, scroll inside |
| Animation | max-height + opacity + translateY; spring open |

**Data on expand (parallel fetch):**

```
GET /intents/digest              → hero card
GET /intents/yesterday           → intent tree roots (+ nested children)
GET /intents/current             → optional "Now" pill on hero
```

**Hero card (digest)**

- Label: `Yesterday · {date}`
- Title: `digest.headline` (e.g. "Building Login Feature")
- Body: `digest.summary`
- Meta: `{intent_count} sessions · {duration}`
- Optional **Now pill** when `current.confidence >= 0.5`: `{current.label}`

**Intent card (each root)**

| Row | Source |
|---|---|
| Title | `intent.label` |
| Project tag | first `project:*` tag, strip prefix |
| Duration | `stats.duration_seconds` → `1h 30m` / `45m` |
| Confidence dot | green ≥0.7, amber ≥0.5, gray below |

**Insight chips** (max 3):

| Chip | Source | Style |
|---|---|---|
| File | `insights.editor[0].file` | blue tint |
| Domain | `insights.browser[0].domain` | green tint |
| Failed shell | `insights.shell[0].command_family` + `count > 0` | red tint |

**Actions per card**

- **Resume** → `POST http://127.0.0.1:9477/v1/restore` with `{ ...intent.resume_payload, mode: "resume" }`
- **Continue** → same with `mode: "continue"`

**Child tree:** Expand caret reveals nested child intents with same chip + action pattern.

**Empty state:** Glass panel — "No sessions found. Seed Role B with POST /pipeline/run-replay."

**Selection:** Keyboard highlights selected card (`border-color: accent`). Flatten tree for ↑/↓ navigation.

### 3.3 Notification toast (morning briefing)

**Purpose:** Passive reminder of yesterday’s work without blocking the desktop.

| Property | Value |
|---|---|
| Position | Bottom-right, 24px inset |
| Width | 320px |
| Shape | Pill (radius 999px) |
| Auto-show | Once per browser session, ~1.5s after app load OR first bar open |
| Auto-dismiss | 12s timeout |
| Persistence | `sessionStorage.intent-os-toast-dismissed` |

**Content**

- Title: `Yesterday · {digest.headline}`
- Body: `digest.summary` (one line, truncate with ellipsis if needed)
- Buttons: **Resume** (top intent from `digest.top_intent_ids[0]`) · **Dismiss**

Resume uses same Role A restore flow as dashboard. Dismiss must not open the bar.

---

## 4. State machine

```
idle ──Ctrl+Space──► barOpen ──focus/type/↓──► expanded
  ▲                      │                         │
  │                      └──Esc────────────────────┘
  │
  └── toast visible (parallel; dismiss → idle)

expanded ──Enter/Resume──► restoring ──200 OK──► idle
```

**Rules**

- Idle: `pointer-events: none` on overlay shell (Electron: ignore mouse events).
- Bar open: capture keyboard; dashboard may be collapsed if input empty.
- Expanded: dashboard visible; search/Copilot results render inside panel.
- Restoring: show compact status toast; close overlay on success.
- Never use blocking `alert()` or full-screen modals.

---

## 5. Liquid-glass design system

MacOS-inspired, dark-first, for Ubuntu overlay.

### 5.1 CSS tokens (use as CSS variables or theme object)

```css
--glass-bg: rgba(20, 22, 28, 0.55);
--glass-border: rgba(255, 255, 255, 0.14);
--glass-highlight: rgba(255, 255, 255, 0.12);
--text-primary: rgba(255, 255, 255, 0.94);
--text-secondary: rgba(255, 255, 255, 0.62);
--text-muted: rgba(255, 255, 255, 0.42);
--accent: #6ea8fe;
--accent-soft: rgba(110, 168, 254, 0.18);
--success: #5dd39e;
--danger: #ff7b72;
--radius-bar: 22px;
--radius-card: 20px;
--radius-pill: 999px;
--shadow-lg: 0 20px 60px rgba(0,0,0,0.35), 0 1px 3px rgba(0,0,0,0.4);
--motion-open: 180ms cubic-bezier(0.2, 0.9, 0.2, 1.05);
--motion-close: 140ms ease;
--motion-hover: 90ms ease;
--font: -apple-system, BlinkMacSystemFont, "Inter", "SF Pro Display", "Cantarell", sans-serif;
```

### 5.2 Glass utility

```css
.glass {
  background: var(--glass-bg);
  backdrop-filter: blur(30px) saturate(180%);
  -webkit-backdrop-filter: blur(30px) saturate(180%);
  border: 1px solid var(--glass-border);
  box-shadow: var(--shadow-lg), inset 0 1px 0 var(--glass-highlight);
}
```

### 5.3 Typography

| Role | Size | Weight | Color |
|---|---|---|---|
| Hero title | 22px | 600 | primary |
| Card title | 15px | 600 | primary |
| Body | 14px | 400 | secondary |
| Meta / chips | 11–12px | 500 | muted |
| Tabular nums | — | — | `font-variant-numeric: tabular-nums` on durations |

Letter-spacing: -0.02em on hero titles only.

### 5.4 Motion

- Respect `prefers-reduced-motion: reduce` — replace springs with 0.01ms fades.
- Hover transitions: 90ms on buttons/chips.
- Mic listening: subtle pulse ring on accent color.

### 5.5 Accessibility

- Focus rings: 2px solid `--accent`, offset 2px on all interactive elements.
- `aria-label` on command input, mic, toast dialog.
- `aria-expanded` on input when dashboard open.
- `role="option"` + `aria-selected` on intent cards.
- Status toasts: `role="status"` + `aria-live="polite"`.

---

## 6. API integration

### 6.1 Base URLs

```typescript
const ROLE_A = 'http://127.0.0.1:9477';
const ROLE_B = 'http://127.0.0.1:9478';
```

Configurable via env / Electron config. No hardcoded secrets in UI.

### 6.2 Role B endpoints (read)

| Endpoint | Use |
|---|---|
| `GET /intents/digest?date=` | Hero + toast (default yesterday) |
| `GET /intents/yesterday` | Dashboard intent list |
| `GET /intents?date=YYYY-MM-DD` | Specific day |
| `GET /intents/{id}` | Detail drill-down (future) |
| `GET /intents/search?q=&limit=` | Command bar search |
| `GET /intents/current` | Now pill (poll ~60s when bar open) |
| `GET /intents/{id}/context` | Copy-agent-context button (future) |
| `POST /copilot/query` | `?` mode |

### 6.3 Role A endpoints (write — user confirmed only)

```http
POST /v1/restore
Content-Type: application/json

{
  "mode": "resume" | "continue",
  "files": ["~/projects/taskflow-app/src/auth.tsx"],
  "urls": ["https://..."],
  "shell": { "cwd": "...", "last_cmd": "..." }
}
```

**Critical:** Copy `resume_payload` from Role B intent JSON verbatim. Only add/override `mode`. Never construct paths from LLM text or Copilot `briefing`.

### 6.4 Error handling

| Condition | UI behavior |
|---|---|
| Role B down | Status toast: "Role B unavailable"; offer mock/offline message |
| Role A restore fails | Status toast: "Restore failed — is Role A running?" |
| Copilot 503 | Inline panel: "Copilot not configured" (not an error state) |
| Copilot 502 | Retryable banner inside dashboard |
| `evidence_status: "insufficient"` | Show empty-evidence message, not error |
| Search empty | Show empty state inside dashboard |

### 6.5 CORS

Role B allows: `localhost:9479`, `127.0.0.1:9479`, `:5173`, `:3000`, `:5000`. Electron `file://` loads should use preload fetch or disable webSecurity only in dev with documented reason.

---

## 7. Recommended production structure

```
role-c/
  design.md                 ← this file
  preview/                  ← validated prototype (do not delete)
    index.html
    mock.json
    serve.sh
  app/                      ← production Electron app (to build)
    package.json
    electron/
      main.ts               ← window, shortcuts, mouse passthrough
      preload.ts            ← expose safe IPC if needed
    src/
      main.tsx
      App.tsx
      components/
        CommandBar.tsx
        SessionDashboard.tsx
        DigestHero.tsx
        IntentCard.tsx
        InsightChips.tsx
        MorningToast.tsx
        StatusToast.tsx
      hooks/
        useOverlayState.ts
        useIntents.ts
        useVoiceInput.ts
      lib/
        api.ts                ← Role A/B clients
        format.ts               ← duration, project tag, chips
        parseMode.ts            ← / ? ! prefix parsing
      styles/
        tokens.css
        glass.css
    index.html
    vite.config.ts
```

### Component responsibilities

| Component | Props / state |
|---|---|
| `CommandBar` | `value`, `onChange`, `onSubmit`, `mode`, `isListening`, `onMicToggle` |
| `SessionDashboard` | `digest`, `intents`, `current`, `selectedIndex`, `expanded` |
| `IntentCard` | `intent`, `selected`, `onResume`, `onContinue`, `onExpand` |
| `MorningToast` | `digest`, `visible`, `onResume`, `onDismiss` |
| `StatusToast` | `message`, `variant: info \| error` |

Port logic from `preview/index.html` script section — do not reimplement inference.

---

## 8. Data types (TypeScript)

Mirror Role B Pydantic models:

```typescript
interface Intent {
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
  stats: { event_count: number; duration_seconds: number; sources: Record<string, number> };
  insights: {
    editor: Array<{ file: string; typed_chars: number; saves: number }>;
    browser: Array<{ domain: string; visits: number }>;
    shell: Array<{ command_family: string; exit_code: number; count: number }>;
  };
  todos: Array<{ path: string; observed_ts: number; marker: string }>;
  resume_payload: ResumePayload;
  children: Intent[];
}

interface ResumePayload {
  files: string[];
  urls: string[];
  shell: { cwd?: string; last_cmd?: string };
}

interface Digest {
  date: string;
  headline: string;
  summary: string;
  top_intent_ids: string[];
  intent_count: number;
  total_duration_seconds: number;
}

interface CurrentIntent {
  label: string;
  summary: string;
  confidence: number;
  since_ts: number;
}
```

---

## 9. What Role C must NOT do

- Read Role A or Role B SQLite files.
- Invent files, URLs, commands, or shell paths.
- Send Copilot-invented payloads to Role A restore.
- Reimplement clustering, labeling, digest generation, or prediction.
- Block the desktop with modal dialogs.
- Upload raw editor text to external APIs.

---

## 10. Acceptance criteria

### Visual

- [ ] Command bar matches preview proportions and glass treatment within 4px tolerance.
- [ ] Dashboard expands from bar with spring animation (or fade when reduced-motion).
- [ ] Toast appears bottom-right without shifting other windows.
- [ ] All interactive elements have visible focus rings.

### Functional

- [ ] `Ctrl+Space` toggles overlay globally (Electron main process).
- [ ] Mock mode works offline via bundled fixture (dev only).
- [ ] Live mode loads digest + yesterday intents from Role B.
- [ ] Resume/Continue posts exact `resume_payload` to Role A.
- [ ] Copilot `?` mode degrades gracefully on 503.
- [ ] Toast dismiss persists for session; Resume triggers restore.
- [ ] Idle overlay does not intercept mouse clicks (passthrough).

### Demo

- [ ] With golden fixture replayed, hero shows **"Building Login Feature"**.
- [ ] Cards show **Edit auth.tsx**, **Debug Npm Command** children.
- [ ] Chips show `auth.tsx`, `developer.mozilla.org`, `npm failed`.
- [ ] Resume opens auth.tsx path from payload (on demo machine with file present).

---

## 11. Development workflow

**Preview (browser):**

```bash
cd role-c/preview
./serve.sh 9479
# Open http://127.0.0.1:9479/?mock=1
```

**Full stack:**

```bash
# Terminal 1 — Role B
cd role-b && .venv/bin/uvicorn intent_engine.api:app --host 127.0.0.1 --port 9478

# Terminal 2 — seed
curl -s -X POST http://127.0.0.1:9478/pipeline/run-replay \
  -H "Content-Type: application/json" \
  --data-binary @role-b/tests/fixtures/demo-day.json

# Terminal 3 — Role C dev
cd role-c/app && npm run dev
```

**Rehearsal gate:** `scripts/demo-rehearsal.sh` at repo root.

---

## 12. Future (post-v1)

- Electron packaging + `.desktop` autostart entry
- `GET /intents/{id}/context` → "Copy for agent" button
- Settings panel: Role A source health (`GET /v1/status`)
- Whisper/local STT replacing Web Speech API
- Wayland compatibility layer (X11 tracker dependency today)
- Light theme variant

---

## 13. Codex implementation prompt (copy-paste)

```
Read role-c/design.md and role-c/preview/index.html.

Build role-c/app as an Electron + React + TypeScript + Vite overlay:
- Port the liquid-glass design tokens and three surfaces (command bar, dashboard, toast).
- Implement the state machine in Section 4.
- Wire Role B read APIs and Role A restore per Section 6.
- Electron main: transparent always-on-top window, Ctrl+Space global shortcut,
  setIgnoreMouseEvents passthrough when idle.
- Do not reimplement Role B inference. Do not invent restore payloads.
- Match acceptance criteria in Section 10.
Use preview/index.html as visual reference; split into components listed in Section 7.
```
