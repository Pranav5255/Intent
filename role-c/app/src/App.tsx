import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { ApiError, roleAApi, roleBApi } from './lib/api';
import { parseCommand } from './lib/parseMode';
import { useIntents } from './hooks/useIntents';
import { useOverlayState } from './hooks/useOverlayState';
import { CommandBar } from './components/CommandBar';
import { MorningToast } from './components/MorningToast';
import { RestoreReview } from './components/RestoreReview';
import { SessionDashboard, type DashboardContent } from './components/SessionDashboard';
import { StatusToast, type StatusMessage } from './components/StatusToast';
import type { CopilotResponse, Intent, RestoreReview as RestoreReviewData } from './types';

function flattenIntents(intents: Intent[]): Intent[] {
  return intents.flatMap((intent) => [intent, ...flattenIntents(intent.children)]);
}

export default function App() {
  const { data, loading, error, load, setData } = useIntents();
  const [morningVisible, setMorningVisible] = useState(false);
  const { open, setOpen, close } = useOverlayState(morningVisible);
  const [value, setValue] = useState('');
  const [content, setContent] = useState<DashboardContent>({ kind: 'timeline' });
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [restoreReview, setRestoreReview] = useState<RestoreReviewData | null>(null);
  const [resumeCandidates, setResumeCandidates] = useState<Parameters<typeof SessionDashboard>[0]['resumeCandidates']>(null);
  const [restoring, setRestoring] = useState(false);
  const [status, setStatus] = useState<StatusMessage | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const statusTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const parsed = useMemo(() => parseCommand(value), [value]);

  const showStatus = useCallback((next: StatusMessage) => {
    if (statusTimer.current) clearTimeout(statusTimer.current);
    setStatus(next);
    statusTimer.current = setTimeout(() => setStatus(null), 6000);
  }, []);

  const closeOverlay = useCallback(() => {
    setValue('');
    setContent({ kind: 'timeline' });
    setRestoreReview(null);
    setResumeCandidates(null);
    close();
  }, [close]);

  const focusInput = useCallback(() => {
    window.setTimeout(() => inputRef.current?.focus(), 30);
  }, []);

  const openOverlay = useCallback(() => {
    setOpen(true);
    void load().catch(() => showStatus({ text: 'Role B unavailable. Check the local service.', variant: 'error' }));
    focusInput();
  }, [focusInput, load, setOpen, showStatus]);

  useEffect(() => {
    if (!open) return;
    void load().catch(() => undefined);
    focusInput();
    const poll = window.setInterval(() => {
      void roleBApi.current()
        .then((current) => setData((previous) => previous ? { ...previous, current } : previous))
        .catch(() => undefined);
    }, 60_000);
    return () => window.clearInterval(poll);
  }, [focusInput, load, open, setData]);

  useEffect(() => {
    if (!data || sessionStorage.getItem('intent-os-toast-dismissed')) return;
    const timeout = window.setTimeout(() => setMorningVisible(true), 1500);
    return () => window.clearTimeout(timeout);
  }, [data]);

  useEffect(() => () => {
    if (statusTimer.current) clearTimeout(statusTimer.current);
  }, []);

  useEffect(() => {
    if (!open || parsed.mode !== 'search') return;
    if (!parsed.query) {
      setContent({ kind: 'timeline' });
      return;
    }
    let active = true;
    const timeout = window.setTimeout(() => {
      setContent({ kind: 'search', query: parsed.query, results: [], loading: true });
      void roleBApi.search(parsed.query)
        .then((results) => {
          if (!active) return;
          setContent({ kind: 'search', query: parsed.query, results, loading: false });
          setSelectedId(results[0]?.id ?? null);
        })
        .catch((reason: unknown) => {
          if (!active) return;
          setContent({ kind: 'search', query: parsed.query, results: [], loading: false });
          showStatus({ text: reason instanceof Error ? reason.message : 'Search is unavailable.', variant: 'error' });
        });
    }, 180);
    return () => {
      active = false;
      window.clearTimeout(timeout);
    };
  }, [open, parsed.mode, parsed.query, showStatus]);

  useEffect(() => {
    if (!data || selectedId) return;
    setSelectedId(flattenIntents(data.intents)[0]?.id ?? null);
  }, [data, selectedId]);

  useEffect(() => {
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key === 'Escape' && open) {
        event.preventDefault();
        if (restoreReview) setRestoreReview(null);
        else closeOverlay();
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [closeOverlay, open, restoreReview]);

  const requestResume = useCallback(async (intentId?: string, query?: string, preferredMode: 'resume' | 'continue' = 'resume') => {
    setResumeCandidates(null);
    try {
      const selection = await roleBApi.selectResume({ intentId, query });
      if (selection.needs_picker) {
        if (selection.candidates.length) setResumeCandidates(selection.candidates);
        else showStatus({ text: 'No stored session matched that restore request.', variant: 'error' });
        return;
      }
      if (!selection.selected) {
        showStatus({ text: 'No stored session matched that restore request.', variant: 'error' });
        return;
      }
      setRestoreReview({
        label: selection.selected.label,
        summary: selection.selected.summary,
        projectTag: selection.selected.project_tag,
        payload: selection.selected.resume_payload,
        preferredMode,
      });
    } catch (reason) {
      showStatus({ text: reason instanceof Error ? reason.message : 'Could not load saved restore context.', variant: 'error' });
    }
  }, [showStatus]);

  const requestCopilot = useCallback(async () => {
    if (!parsed.query) return;
    setContent({ kind: 'copilot', question: parsed.query, response: null, loading: true, unavailable: false });
    try {
      const response = await roleBApi.askCopilot(parsed.query);
      setContent({ kind: 'copilot', question: parsed.query, response, loading: false, unavailable: false });
    } catch (reason) {
      const unavailable = reason instanceof ApiError && reason.status === 503;
      setContent({ kind: 'copilot', question: parsed.query, response: null, loading: false, unavailable });
      if (!unavailable) showStatus({ text: 'Copilot is temporarily unavailable. Press Enter to retry.', variant: 'error' });
    }
  }, [parsed.query, showStatus]);

  const reviewCopilotProposal = useCallback((response: CopilotResponse) => {
    const proposal = response.resume_proposal;
    if (!proposal) return;
    const citation = response.citations.find((item) => item.intent_id === proposal.intent_id);
    setRestoreReview({
      label: citation?.label ?? 'Stored restore context',
      summary: proposal.briefing ?? citation?.summary ?? 'Review the stored restore context before opening it.',
      payload: proposal.resume_payload,
      preferredMode: 'resume',
    });
  }, []);

  const confirmRestore = useCallback(async (mode: 'resume' | 'continue') => {
    if (!restoreReview) return;
    setRestoring(true);
    try {
      const result = await roleAApi.restore({ ...restoreReview.payload, mode });
      setRestoreReview(null);
      if (result.ok) {
        showStatus({ text: 'Saved context opened locally.', variant: 'success' });
        closeOverlay();
      } else {
        showStatus({ text: `Restore finished with issues: ${result.failed.join(' ')}`, variant: 'error' });
      }
    } catch (reason) {
      showStatus({ text: reason instanceof Error ? reason.message : 'Restore failed — is Role A running?', variant: 'error' });
    } finally {
      setRestoring(false);
    }
  }, [closeOverlay, restoreReview, showStatus]);

  const selectionIds = useMemo(() => {
    if (content.kind === 'search') return content.results.map((result) => result.id);
    if (content.kind === 'timeline' && data) return flattenIntents(data.intents).map((intent) => intent.id);
    return [];
  }, [content, data]);

  const moveSelection = useCallback((offset: number) => {
    if (!selectionIds.length) return;
    const current = Math.max(0, selectionIds.indexOf(selectedId ?? ''));
    const next = (current + offset + selectionIds.length) % selectionIds.length;
    setSelectedId(selectionIds[next]);
  }, [selectedId, selectionIds]);

  const handleInputKey = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'Escape') {
      event.preventDefault();
      closeOverlay();
      return;
    }
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      moveSelection(event.key === 'ArrowDown' ? 1 : -1);
      return;
    }
    if (event.key !== 'Enter') return;
    event.preventDefault();
    if (parsed.mode === 'copilot') {
      void requestCopilot();
      return;
    }
    void requestResume(selectedId ?? undefined, parsed.mode === 'restore' ? parsed.query : undefined);
  };

  const dismissMorning = () => {
    sessionStorage.setItem('intent-os-toast-dismissed', 'true');
    setMorningVisible(false);
  };

  const resumeMorning = () => {
    dismissMorning();
    openOverlay();
    void requestResume(data?.digest.top_intent_ids[0]);
  };

  const inputBusy = loading || content.kind === 'search' && content.loading || content.kind === 'copilot' && content.loading;
  return (
    <main className={`overlay-shell ${open ? 'is-open' : 'is-idle'}`}>
      <div className="command-region">
        <CommandBar
          ref={inputRef}
          value={value}
          mode={parsed.mode}
          open={open}
          expanded={open}
          busy={inputBusy}
          onChange={setValue}
          onKeyDown={handleInputKey}
          onOpen={openOverlay}
          onClose={closeOverlay}
        />
        {open && (
          <SessionDashboard
            data={data}
            content={content}
            loading={loading}
            error={error}
            selectedId={selectedId}
            resumeCandidates={resumeCandidates}
            onRetry={() => void load().catch(() => undefined)}
            onSelect={setSelectedId}
            onResume={(intentId, preferredMode = 'resume') => void requestResume(intentId, undefined, preferredMode)}
            onCopilotResume={reviewCopilotProposal}
          />
        )}
        {restoreReview && (
          <RestoreReview
            review={restoreReview}
            restoring={restoring}
            onClose={() => setRestoreReview(null)}
            onConfirm={(mode) => void confirmRestore(mode)}
          />
        )}
      </div>
      {data && <MorningToast digest={data.digest} visible={morningVisible} onResume={resumeMorning} onDismiss={dismissMorning} />}
      <StatusToast status={status} />
    </main>
  );
}
