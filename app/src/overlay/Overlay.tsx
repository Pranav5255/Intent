import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type MouseEvent as ReactMouseEvent } from 'react';
import { Icon } from '../components/Icon';
import { RestoreReview, type ReviewTarget } from '../components/RestoreReview';
import { ConfirmDialog, Empty, Keys, Snippet, StatusDot, Toasts, type ConfirmSpec } from '../components/ui';
import { api, ApiError } from '../lib/api';
import { copyText, hideOverlay, listen, openMain } from '../lib/bridge';
import { modHeld } from '../lib/env';
import { ago, daysAgoIso, duration, relativeDay, todayIso } from '../lib/format';
import { fuzzyFilter, fuzzyRank } from '../lib/fuzzy';
import { byRecency, flatten, hasResume, laneFor, projectOf } from '../lib/intent';
import { restoreSelection, selectAll, type RestoreSelection } from '../lib/restore';
import {
  errorText,
  pauseCapture,
  pausedUntilLabel,
  resumeCapture,
  summarizeCapture,
  toast,
  updateToast,
  useCapture,
} from '../lib/store';
import type { CitedIntent, Intent, SearchHit } from '../lib/types';
import { ActionPanel } from './ActionPanel';
import { commandList, defaultCommands, itemActions, type Command, type ItemActionId } from './commands';
import { CopilotPanel, type CopilotState } from './CopilotAnswer';
import { CommandPreview, IntentPreview, PreviewLoading } from './PreviewPane';

type Section = 'Resume' | 'Today' | 'Earlier' | 'Matches' | 'Actions' | 'Projects';

type Row =
  | { key: string; section: Section; kind: 'intent'; intent: Intent }
  | { key: string; section: Section; kind: 'hit'; hit: SearchHit }
  | { key: string; section: Section; kind: 'command'; command: Command }
  | { key: string; section: Section; kind: 'ask'; question: string }
  | { key: string; section: Section; kind: 'project'; name: string; count: number };

type Mode = 'search' | 'ask' | 'commands' | 'project';

function parse(raw: string): { mode: Mode; text: string } {
  const t = raw.trimStart();
  if (t.startsWith('?')) return { mode: 'ask', text: t.slice(1).trimStart() };
  if (t.startsWith('>')) return { mode: 'commands', text: t.slice(1).trimStart() };
  if (t.startsWith('@')) return { mode: 'project', text: t.slice(1) };
  return { mode: 'search', text: t };
}

const SEARCH_WINDOW_DAYS = 14;

function useWidth(): number {
  const [w, setW] = useState(() => window.innerWidth);
  useEffect(() => {
    const on = () => setW(window.innerWidth);
    window.addEventListener('resize', on);
    return () => window.removeEventListener('resize', on);
  }, []);
  return w;
}

export function Overlay() {
  const capture = useCapture();
  const summary = summarizeCapture(capture);
  const paused = Boolean(capture.status?.capture_paused);
  const width = useWidth();
  const [previewOn, setPreviewOn] = useState(true);
  const showPreview = previewOn && width >= 860;

  const [raw, setRaw] = useState('');
  const [scope, setScope] = useState<string | null>(null);
  const [index, setIndex] = useState(0);
  const [recent, setRecent] = useState<Intent[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [offline, setOffline] = useState(false);
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [searching, setSearching] = useState(false);
  const [cache, setCache] = useState<Record<string, Intent>>({});
  const [selections, setSelections] = useState<Record<string, RestoreSelection>>({});
  const [copilot, setCopilot] = useState<CopilotState>({ status: 'idle' });
  const [citation, setCitation] = useState(0);
  const [review, setReview] = useState<ReviewTarget | null>(null);
  const [panel, setPanel] = useState(false);
  const [confirm, setConfirm] = useState<ConfirmSpec | null>(null);
  const [opening, setOpening] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const { mode, text } = parse(raw);

  // ------------------------------------------------------------------ data

  const load = useCallback(async () => {
    try {
      const [today, yesterday] = await Promise.all([api.intentsFor(todayIso()), api.intentsFor(daysAgoIso(1))]);
      const all = [...today, ...yesterday];
      setRecent(all);
      setCache((prev) => {
        const next = { ...prev };
        for (const i of flatten(all)) next[i.id] = i;
        return next;
      });
      setOffline(false);
    } catch (reason) {
      setOffline(reason instanceof ApiError && reason.unreachable);
    } finally {
      setLoaded(true);
    }
  }, []);

  const reset = useCallback(() => {
    setRaw('');
    setScope(null);
    setIndex(0);
    setHits([]);
    setCopilot({ status: 'idle' });
    setReview(null);
    setPanel(false);
    setConfirm(null);
    setOpening((n) => n + 1);
    window.setTimeout(() => inputRef.current?.focus(), 0);
  }, []);

  useEffect(() => {
    void load();
    inputRef.current?.focus();
    const unlisten = listen('overlay-shown', () => {
      reset();
      void load();
    });
    return () => void unlisten.then((fn) => fn());
  }, [load, reset]);

  // Each open remounts the dialog (`key={opening}`), so the input that reset()
  // focused may be the one being thrown away; focus the new one once it exists.
  useEffect(() => {
    inputRef.current?.focus();
  }, [opening]);

  // Hybrid search on the daemon, debounced. Local lists filter instantly.
  useEffect(() => {
    if (mode !== 'search' || text.trim().length < 2) {
      setHits([]);
      setSearching(false);
      return;
    }
    let live = true;
    setSearching(true);
    const timer = window.setTimeout(() => {
      api
        .search(text.trim(), { limit: 12, date_from: daysAgoIso(SEARCH_WINDOW_DAYS), project: scope ?? undefined })
        .then((result) => live && setHits(result))
        .catch(() => live && setHits([]))
        .finally(() => live && setSearching(false));
    }, 120);
    return () => {
      live = false;
      window.clearTimeout(timer);
    };
  }, [mode, text, scope]);

  // ------------------------------------------------------------------ rows

  const roots = useMemo(() => {
    const list = byRecency(recent.filter((i) => i.depth === 0));
    return scope ? list.filter((i) => projectOf(i) === scope) : list;
  }, [recent, scope]);

  const projects = useMemo(() => {
    const counts = new Map<string, number>();
    for (const i of recent) {
      const p = projectOf(i);
      if (p) counts.set(p, (counts.get(p) ?? 0) + 1);
    }
    return [...counts.entries()].map(([name, count]) => ({ name, count }));
  }, [recent]);

  const rows: Row[] = useMemo(() => {
    const out: Row[] = [];
    if (mode === 'project') {
      for (const p of fuzzyRank(projects, text, (x) => x.name)) {
        out.push({ key: `p:${p.name}`, section: 'Projects', kind: 'project', name: p.name, count: p.count });
      }
      return out;
    }
    if (mode === 'commands') {
      for (const c of fuzzyRank(commandList(paused), text, (x) => `${x.title} ${x.keywords}`)) {
        out.push({ key: `c:${c.id}`, section: 'Actions', kind: 'command', command: c });
      }
      return out;
    }
    if (mode === 'ask') return out;

    const today = todayIso();
    const matching = fuzzyFilter(roots, text, (i) => `${i.label} ${projectOf(i) ?? ''}`);
    const resumable = matching.filter(hasResume).slice(0, 3);
    const resumeIds = new Set(resumable.map((i) => i.id));
    const rest = matching.filter((i) => !resumeIds.has(i.id));
    for (const i of resumable) out.push({ key: `i:${i.id}`, section: 'Resume', kind: 'intent', intent: i });
    for (const i of rest.filter((x) => x.date === today)) out.push({ key: `i:${i.id}`, section: 'Today', kind: 'intent', intent: i });
    if (!text.trim()) {
      for (const i of rest.filter((x) => x.date !== today).slice(0, 2)) {
        out.push({ key: `i:${i.id}`, section: 'Earlier', kind: 'intent', intent: i });
      }
    }
    const shown = new Set(out.map((r) => (r.kind === 'intent' ? r.intent.id : '')));
    for (const h of hits) {
      if (shown.has(h.id)) continue;
      const known = cache[h.id];
      if (scope && known && projectOf(known) !== scope) continue;
      out.push({ key: `h:${h.id}`, section: 'Matches', kind: 'hit', hit: h });
    }
    if (text.trim()) {
      out.push({ key: 'ask', section: 'Actions', kind: 'ask', question: text.trim() });
      for (const c of fuzzyRank(commandList(paused), text, (x) => `${x.title} ${x.keywords}`).slice(0, 3)) {
        out.push({ key: `c:${c.id}`, section: 'Actions', kind: 'command', command: c });
      }
    } else {
      for (const c of defaultCommands(paused)) out.push({ key: `c:${c.id}`, section: 'Actions', kind: 'command', command: c });
    }
    return out;
  }, [mode, text, roots, hits, paused, projects, cache, scope]);

  useEffect(() => setIndex(0), [raw, scope]);
  const selected: Row | undefined = rows[Math.min(index, rows.length - 1)];

  // Keep the selected row in view (instant, no smooth scroll).
  useEffect(() => {
    const el = listRef.current?.querySelector<HTMLElement>('[data-selected="true"]');
    el?.scrollIntoView({ block: 'nearest' });
  }, [index, rows]);

  // Resolve the full intent for a search hit lazily (preview + actions need it).
  const selectedIntentId =
    selected?.kind === 'intent' ? selected.intent.id : selected?.kind === 'hit' ? selected.hit.id : null;
  useEffect(() => {
    if (!selectedIntentId || cache[selectedIntentId]) return;
    let live = true;
    api
      .intent(selectedIntentId)
      .then((i) => live && setCache((prev) => ({ ...prev, [i.id]: i })))
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [selectedIntentId, cache]);
  const selectedIntent = selectedIntentId ? (cache[selectedIntentId] ?? null) : null;

  const selectionFor = useCallback(
    (intent: Intent) => selections[intent.id] ?? selectAll(intent.resume_payload),
    [selections],
  );

  // ------------------------------------------------------------------ actions

  const finish = useCallback(() => {
    void hideOverlay();
  }, []);

  const toReview = (intent: Intent, only?: 'tabs'): ReviewTarget => ({
    id: intent.id,
    label: intent.label,
    summary: intent.summary,
    tags: intent.tags,
    end_ts: intent.end_ts,
    duration_seconds: intent.stats.duration_seconds,
    payload: intent.resume_payload,
    insights: intent.insights,
    only,
  });

  const resume = async (intent: Intent) => {
    // The preview pane *is* the review when visible; otherwise go through review.
    if (!showPreview) {
      setReview(toReview(intent));
      return;
    }
    const ok = await restoreSelection(intent.label, intent.resume_payload, selectionFor(intent));
    if (ok) finish();
  };

  const runCommand = async (c: Command) => {
    try {
      switch (c.id) {
        case 'ask':
          setRaw('? ');
          return;
        case 'pause-15m':
        case 'pause-1h':
        case 'pause-tomorrow': {
          const until = await pauseCapture(c.id === 'pause-15m' ? '15m' : c.id === 'pause-1h' ? '1h' : 'tomorrow');
          toast({ tone: 'neutral', text: `Capture paused until ${pausedUntilLabel(until)}`, detail: 'Nothing is recorded while paused.' });
          setRaw('');
          return;
        }
        case 'resume-capture':
          await resumeCapture();
          toast({ tone: 'success', text: 'Capture resumed' });
          setRaw('');
          return;
        case 'run-pipeline': {
          const id = toast({ tone: 'progress', text: 'Grouping today’s activity…' });
          try {
            await api.runPipeline(todayIso());
            updateToast(id, { tone: 'success', text: 'Today’s threads are up to date' });
            void load();
          } catch (reason) {
            updateToast(id, { tone: 'error', text: 'Couldn’t process today', detail: errorText(reason) });
          }
          return;
        }
        case 'open-today':
          await openMain('today');
          break;
        case 'open-timeline':
          await openMain('timeline');
          break;
        case 'open-search':
          await openMain('search');
          break;
        case 'open-settings':
          await openMain('settings');
          break;
      }
      finish();
    } catch (reason) {
      toast({ tone: 'error', text: 'That didn’t work', detail: errorText(reason) });
    }
  };

  const ask = async (question: string) => {
    if (!question.trim()) return;
    setCopilot({ status: 'loading', question: question.trim() });
    setCitation(0);
    try {
      const response = await api.copilot({ question: question.trim(), mode: 'auto', date_from: daysAgoIso(SEARCH_WINDOW_DAYS), date_to: todayIso() });
      setCopilot({ status: 'done', question: question.trim(), response });
    } catch (reason) {
      if (reason instanceof ApiError && reason.status === 503) setCopilot({ status: 'unavailable', question: question.trim() });
      else setCopilot({ status: 'error', question: question.trim(), message: errorText(reason) });
    }
  };

  const openCitation = async (c: CitedIntent) => {
    await openMain(`intent:${c.intent_id}`);
    finish();
  };

  const runItemAction = async (id: ItemActionId, intent: Intent) => {
    setPanel(false);
    const project = projectOf(intent);
    switch (id) {
      case 'resume':
        await resume(intent);
        return;
      case 'review':
        setReview(toReview(intent));
        return;
      case 'tabs-only':
        setReview(toReview(intent, 'tabs'));
        return;
      case 'open':
        await openMain(`intent:${intent.id}`);
        finish();
        return;
      case 'copy-summary':
        await copyText(`${intent.label}\n\n${intent.summary}`);
        toast({ tone: 'success', text: 'Summary copied' });
        return;
      case 'copy-context':
        try {
          const ctx = await api.context(intent.id);
          await copyText(ctx.markdown);
          toast({ tone: 'success', text: 'Context copied as Markdown', detail: 'Paste it into a chat or a note.' });
        } catch (reason) {
          toast({ tone: 'error', text: 'Couldn’t build the context', detail: errorText(reason) });
        }
        return;
      case 'forget-day':
        setConfirm({
          title: `Forget ${relativeDay(intent.date).toLowerCase() === 'today' ? 'today' : relativeDay(intent.date)}?`,
          text: 'Everything recorded on this day is deleted from this computer, with its threads and search entries. This can’t be undone.',
          confirm: 'Forget day',
          destructive: true,
          onConfirm: async () => {
            try {
              await api.forgetDate(intent.date);
              toast({ tone: 'success', text: `Forgot ${relativeDay(intent.date)}` });
              void load();
            } catch (reason) {
              toast({ tone: 'error', text: 'Couldn’t forget that day', detail: errorText(reason) });
            }
          },
        });
        return;
      case 'forget-project':
        if (!project) return;
        setConfirm({
          title: `Forget everything about “${project}”?`,
          text: 'The project’s activity and threads are deleted from this computer, on every day. Days that mixed it with other work are rebuilt without it. This can’t be undone.',
          confirm: 'Forget project',
          destructive: true,
          onConfirm: async () => {
            try {
              await api.forgetProject(project);
              toast({ tone: 'success', text: `Forgot ${project}` });
              void load();
            } catch (reason) {
              toast({ tone: 'error', text: 'Couldn’t forget that project', detail: errorText(reason) });
            }
          },
        });
        return;
    }
  };

  const activate = async (row: Row | undefined, withMod: boolean) => {
    if (!row) return;
    if (row.kind === 'command') return runCommand(row.command);
    if (row.kind === 'ask') {
      setRaw(`? ${row.question}`);
      void ask(row.question);
      return;
    }
    if (row.kind === 'project') {
      setScope(row.name);
      setRaw('');
      return;
    }
    const intent = row.kind === 'intent' ? row.intent : cache[row.hit.id];
    if (!intent) return;
    if (withMod) setReview(toReview(intent));
    else await resume(intent);
  };

  // ------------------------------------------------------------------ keyboard

  const onKeyDown = (e: ReactKeyboardEvent<HTMLInputElement>) => {
    const mod = modHeld(e);
    const citations = copilot.status === 'done' ? copilot.response.citations : [];
    const inAnswer = mode === 'ask' && copilot.status === 'done' && copilot.question === text.trim();

    if (e.key === 'Escape') {
      e.preventDefault();
      if (raw || scope) {
        setRaw('');
        if (!raw) setScope(null);
        setCopilot({ status: 'idle' });
      } else finish();
      return;
    }
    if (e.key === 'ArrowDown' || (e.ctrlKey && e.key === 'n')) {
      e.preventDefault();
      if (inAnswer) setCitation((i) => Math.min(citations.length - 1, i + 1));
      else setIndex((i) => (rows.length ? (i + 1) % rows.length : 0));
      return;
    }
    if (e.key === 'ArrowUp' || (e.ctrlKey && e.key === 'p' && !mod)) {
      e.preventDefault();
      if (inAnswer) setCitation((i) => Math.max(0, i - 1));
      else setIndex((i) => (rows.length ? (i - 1 + rows.length) % rows.length : 0));
      return;
    }
    if (e.key === 'Tab') {
      e.preventDefault();
      if (mode === 'project' && selected?.kind === 'project') {
        setScope(selected.name);
        setRaw('');
      }
      return;
    }
    if (e.key === 'Backspace' && !raw && scope) {
      e.preventDefault();
      setScope(null);
      return;
    }
    if (mod && e.key.toLowerCase() === 'k') {
      e.preventDefault();
      if (selectedIntent) setPanel(true);
      return;
    }
    if (mod && e.key === '.') {
      e.preventDefault();
      setPreviewOn((v) => !v);
      return;
    }
    if (mod && e.key.toLowerCase() === 'o' && selectedIntent) {
      e.preventDefault();
      void runItemAction('open', selectedIntent);
      return;
    }
    if (mod && e.key.toLowerCase() === 'c' && selectedIntent && inputRef.current?.selectionStart === inputRef.current?.selectionEnd) {
      e.preventDefault();
      void runItemAction(e.shiftKey ? 'copy-context' : 'copy-summary', selectedIntent);
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      if (mode === 'ask') {
        if (inAnswer && citations[citation]) {
          const c = citations[citation]!;
          const known = cache[c.intent_id];
          if (mod && known) setReview(toReview(known));
          else void openCitation(c);
        } else void ask(text);
        return;
      }
      void activate(selected, mod);
    }
  };

  // ------------------------------------------------------------------ render

  const footerHints = (): Array<[string, string[]]> => {
    if (review) return [];
    if (mode === 'ask') {
      return copilot.status === 'done' && copilot.question === text.trim()
        ? [['Open source', ['↵']], ['Review', ['mod', '↵']]]
        : [['Ask', ['↵']]];
    }
    if (!selected) return [];
    if (selected.kind === 'command') return [['Run', ['↵']]];
    if (selected.kind === 'ask') return [['Ask', ['↵']]];
    if (selected.kind === 'project') return [['Scope to project', ['Tab']]];
    return [
      ['Resume', ['↵']],
      ['Review', ['mod', '↵']],
      ['Actions', ['mod', 'K']],
    ];
  };

  let lastSection: Section | null = null;
  const placeholder =
    mode === 'ask'
      ? 'Ask about your work…'
      : scope
        ? `Search ${scope}…`
        : 'Resume, search, or type ? to ask, > for commands, @ for a project';

  const showEmpty =
    loaded && mode === 'search' && text.trim().length >= 2 && !searching && rows.every((r) => r.kind === 'ask' || r.kind === 'command');

  return (
    <div className="ov-stage" onMouseDown={(e) => e.target === e.currentTarget && finish()}>
      <div key={opening} className={`ov ${showPreview && !review ? 'has-preview' : ''}`} role="dialog" aria-label="Intent">
        <div className="ov-input-row">
          {mode === 'ask' ? (
            <Icon name="ask" className="ov-input-icon is-ask" size={18} />
          ) : mode === 'commands' ? (
            <Icon name="command" className="ov-input-icon" size={18} />
          ) : (
            <Icon name="search" className="ov-input-icon" size={18} />
          )}
          {scope && (
            <span className="scope-chip">
              <span className="lane-dot" style={{ background: laneFor({ tags: [`project:${scope}`], label: scope }) }} />
              {scope}
            </span>
          )}
          <input
            ref={inputRef}
            className="ov-input"
            value={raw}
            placeholder={placeholder}
            spellCheck={false}
            autoComplete="off"
            onChange={(e) => setRaw(e.target.value)}
            onKeyDown={onKeyDown}
            aria-label="Search your work"
            aria-controls="ov-list"
            aria-activedescendant={selected ? `row-${selected.key}` : undefined}
            disabled={Boolean(review) || panel}
          />
          {(searching || copilot.status === 'loading') && <span className="spinner" />}
          {mode !== 'search' && <span className="ov-mode mono t3">{mode === 'ask' ? 'Ask' : mode === 'commands' ? 'Commands' : 'Project'}</span>}
        </div>

        {review ? (
          <RestoreReview
            variant="inline"
            target={review}
            onCancel={() => {
              setReview(null);
              window.setTimeout(() => inputRef.current?.focus(), 0);
            }}
            onDone={finish}
          />
        ) : (
          <div className="ov-body">
            <div className="ov-list" id="ov-list" role="listbox" ref={listRef}>
              {offline && (
                <Empty icon="alert" title="Intent isn’t running" text="Start it from the app menu, or run “systemctl --user start intentd”." />
              )}
              {mode === 'ask' && (
                <CopilotPanel
                  state={copilot}
                  question={text}
                  selectedCitation={citation}
                  onHoverCitation={setCitation}
                  onCite={(c) => void openCitation(c)}
                  onResume={(id) => {
                    const known = cache[id];
                    if (known) setReview(toReview(known));
                    else void api.intent(id).then((i) => setReview(toReview(i)));
                  }}
                  citationLane={(c) => (cache[c.intent_id] ? laneFor(cache[c.intent_id]!) : null)}
                />
              )}
              {!offline &&
                mode !== 'ask' &&
                rows.map((row, i) => {
                  const header = row.section !== lastSection ? row.section : null;
                  lastSection = row.section;
                  const isSel = i === index;
                  return (
                    <div key={row.key}>
                      {header && (
                        <div className="section-label">
                          {header}
                          {header === 'Matches' && <span className="t3"> · last {SEARCH_WINDOW_DAYS} days</span>}
                        </div>
                      )}
                      <RowView
                        row={row}
                        selected={isSel}
                        onHover={() => setIndex(i)}
                        onClick={(e) => {
                          setIndex(i);
                          void activate(row, modHeld(e));
                        }}
                      />
                    </div>
                  );
                })}
              {showEmpty && (
                <Empty
                  icon="search"
                  title={`Nothing matched “${text.trim()}” in the last ${SEARCH_WINDOW_DAYS} days`}
                  text="Try fewer words, or ask in plain language."
                >
                  <button className="btn btn-sm" onClick={() => setRaw(`? ${text.trim()}`)}>
                    Ask instead <Keys keys={['?']} />
                  </button>
                </Empty>
              )}
              {loaded && !offline && mode === 'search' && !text && rows.every((r) => r.kind === 'command') && (
                <Empty icon="today" title="No threads yet today" text="Intent groups your activity a few minutes after you start working." />
              )}
            </div>
            {showPreview && (
              <aside className="ov-preview" aria-label="Preview">
                {selected?.kind === 'command' ? (
                  <CommandPreview command={selected.command} />
                ) : selected?.kind === 'ask' && mode === 'search' ? (
                  <CommandPreview command={commandList(paused).find((c) => c.id === 'ask')!} />
                ) : selectedIntent ? (
                  <IntentPreview
                    intent={selectedIntent}
                    selection={selectionFor(selectedIntent)}
                    onSelection={(s) => setSelections((prev) => ({ ...prev, [selectedIntent.id]: s }))}
                  />
                ) : selectedIntentId ? (
                  <PreviewLoading />
                ) : mode === 'ask' && copilot.status === 'done' && copilot.response.citations[citation] && cache[copilot.response.citations[citation]!.intent_id] ? (
                  <IntentPreview
                    intent={cache[copilot.response.citations[citation]!.intent_id]!}
                    selection={selectionFor(cache[copilot.response.citations[citation]!.intent_id]!)}
                    onSelection={(s) =>
                      setSelections((prev) => ({ ...prev, [copilot.response.citations[citation]!.intent_id]: s }))
                    }
                  />
                ) : (
                  <div className="pv pv-blank t3">
                    {mode === 'ask' ? 'Sources appear here.' : mode === 'project' ? 'Scope search to one project.' : ''}
                  </div>
                )}
              </aside>
            )}
          </div>
        )}

        <footer className="ov-foot">
          <span className={`ov-status is-${summary.tone}`}>
            <StatusDot tone={summary.tone} />
            {summary.text}
          </span>
          <span className="ov-hints">
            {footerHints().map(([label, keys]) => (
              <span key={label} className="ov-hint">
                <Keys keys={keys} />
                <span>{label}</span>
              </span>
            ))}
          </span>
        </footer>

        {panel && selectedIntent && (
          <ActionPanel
            title={selectedIntent.label}
            actions={itemActions({
              project: projectOf(selectedIntent),
              hasTabs: selectedIntent.resume_payload.urls.length > 0,
              date: selectedIntent.date,
            })}
            onClose={() => {
              setPanel(false);
              window.setTimeout(() => inputRef.current?.focus(), 0);
            }}
            onRun={(id) => void runItemAction(id, selectedIntent)}
          />
        )}
        {confirm && (
          <ConfirmDialog
            spec={confirm}
            onClose={() => {
              setConfirm(null);
              window.setTimeout(() => inputRef.current?.focus(), 0);
            }}
          />
        )}
        <Toasts />
      </div>
    </div>
  );
}

function RowView({
  row,
  selected,
  onHover,
  onClick,
}: {
  row: Row;
  selected: boolean;
  onHover: () => void;
  onClick: (e: ReactMouseEvent) => void;
}) {
  const common = {
    id: `row-${row.key}`,
    role: 'option' as const,
    'aria-selected': selected,
    'data-selected': selected,
    className: `row ${selected ? 'is-selected' : ''}`,
    onMouseMove: onHover,
    onClick,
  };
  if (row.kind === 'intent') {
    const i = row.intent;
    const project = projectOf(i);
    return (
      <div {...common}>
        <span className="lane-swatch" style={{ background: laneFor(i) }} />
        <span className="row-label ellipsis">{i.label}</span>
        {project && <span className="row-project">{project}</span>}
        <span className="row-meta mono">
          {ago(i.end_ts)} · {duration(i.stats.duration_seconds)}
        </span>
      </div>
    );
  }
  if (row.kind === 'hit') {
    const h = row.hit;
    return (
      <div {...common} className={`${common.className} row-tall`}>
        <Icon name="search" size={14} className="row-icon" />
        <span className="row-stack">
          <span className="row-label ellipsis">{h.label}</span>
          <span className="row-snippet ellipsis">
            <Snippet text={h.highlight_snippet || h.summary} />
          </span>
        </span>
        <span className="row-meta mono">{relativeDay(h.date)}</span>
      </div>
    );
  }
  if (row.kind === 'command') {
    const c = row.command;
    return (
      <div {...common}>
        <Icon name={c.icon} size={15} className="row-icon" />
        <span className="row-label ellipsis">{c.title}</span>
        {c.hint && <span className="row-meta mono">{c.hint}</span>}
      </div>
    );
  }
  if (row.kind === 'ask') {
    return (
      <div {...common}>
        <Icon name="ask" size={15} className="row-icon" />
        <span className="row-label ellipsis">
          Ask “<span className="t2">{row.question}</span>”
        </span>
        <span className="row-meta">
          <Keys keys={['?']} />
        </span>
      </div>
    );
  }
  return (
    <div {...common}>
      <span className="lane-swatch" style={{ background: laneFor({ tags: [`project:${row.name}`], label: row.name }) }} />
      <span className="row-label ellipsis">{row.name}</span>
      <span className="row-meta mono">{row.count} recent</span>
    </div>
  );
}
