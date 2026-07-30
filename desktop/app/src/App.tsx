import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { ApiError, roleAApi, roleBApi } from './lib/api';
import { parseCommand } from './lib/parseMode';
import { useIntents } from './hooks/useIntents';
import { useOverlayState } from './hooks/useOverlayState';
import { CommandBar } from './components/CommandBar';
import { RestoreReview } from './components/RestoreReview';
import { SessionDashboard, type DashboardContent } from './components/SessionDashboard';
import { SettingsPanel } from './components/SettingsPanel';
import { StatusToast, type StatusMessage } from './components/StatusToast';
import { WelcomeScreen } from './components/WelcomeScreen';
import type { CopilotResponse, DetailedCaptureSettings, DetailedCaptureSettingsUpdate, Intent, IntelligencePreviewSample, LLMSettings, LLMSettingsUpdate, RestoreReview as RestoreReviewData, RetentionPolicy, SettingsTab } from './types';

const ONBOARDING_STORAGE_KEY = 'intent-onboarding-complete-v1';

function flattenIntents(intents: Intent[]): Intent[] {
  return intents.flatMap((intent) => [intent, ...flattenIntents(intent.children)]);
}

export default function App() {
  const { data, loading, error, load, setData } = useIntents();
  const [onboardingStep, setOnboardingStep] = useState<number | null>(() => {
    try {
      return window.localStorage.getItem(ONBOARDING_STORAGE_KEY) ? null : 0;
    } catch {
      return 0;
    }
  });
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsTab, setSettingsTab] = useState<SettingsTab>('capture');
  const [settings, setSettings] = useState<LLMSettings | null>(null);
  const [captureSettings, setCaptureSettings] = useState<DetailedCaptureSettings | null>(null);
  const [retentionPolicy, setRetentionPolicy] = useState<RetentionPolicy | null>(null);
  const [filesystemAccessible, setFilesystemAccessible] = useState(false);
  const [detailedEventCount, setDetailedEventCount] = useState(0);
  const [intelligencePreview, setIntelligencePreview] = useState<IntelligencePreviewSample | null>(null);
  const [settingsLoading, setSettingsLoading] = useState(false);
  const [settingsSaving, setSettingsSaving] = useState(false);
  const [settingsError, setSettingsError] = useState<string | null>(null);
  const onboardingVisible = onboardingStep !== null;
  const electronAvailable = Boolean(window.intent);
  const settingsAvailable = electronAvailable || import.meta.env.DEV;
  const productionSettingsAvailable = electronAvailable && !import.meta.env.DEV;
  const { open, setOpen, close } = useOverlayState(onboardingVisible || settingsOpen);
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
    setSettingsOpen(false);
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

  const openSettings = useCallback(async (tab: SettingsTab = 'capture') => {
    if (!settingsAvailable) return;
    setRestoreReview(null);
    setResumeCandidates(null);
    setSettingsOpen(true);
    setSettingsTab(tab);
    setSettingsError(null);
    setSettingsLoading(true);
    setOpen(true);
    try {
      const [capture, filesystem, retention, status] = await Promise.all([
        roleAApi.captureConfig(),
        roleAApi.filesystemCapture(),
        roleAApi.retentionPolicy(),
        roleAApi.captureStatus(),
      ]);
      setCaptureSettings(capture);
      setFilesystemAccessible(filesystem.all_accessible);
      setRetentionPolicy(retention);
      const counts = status.detailed_capture?.event_counts ?? {};
      setDetailedEventCount(Object.values(counts).reduce((sum, value) => sum + (typeof value === 'number' ? value : 0), 0));
      if (productionSettingsAvailable) {
        setSettings(await roleBApi.llmSettings());
      }
    } catch (reason) {
      setSettingsError(reason instanceof Error ? reason.message : 'Could not load local settings.');
    } finally {
      setSettingsLoading(false);
    }
  }, [productionSettingsAvailable, setOpen, settingsAvailable]);

  const saveSettings = useCallback(async (update: LLMSettingsUpdate) => {
    if (!productionSettingsAvailable) {
      setSettingsError('Copilot provider settings require the installed Intent desktop app.');
      return;
    }
    setSettingsSaving(true);
    setSettingsError(null);
    try {
      const saved = await roleBApi.saveLlmSettings(update);
      setSettings(saved);
      showStatus({ text: saved.copilot_enabled ? 'Intelligence settings saved locally.' : 'Provider saved. Copilot remains off.', variant: 'success' });
    } catch (reason) {
      setSettingsError(reason instanceof Error ? reason.message : 'Could not save local provider settings.');
    } finally {
      setSettingsSaving(false);
    }
  }, [productionSettingsAvailable, showStatus]);

  const saveCaptureSettings = useCallback(async (update: DetailedCaptureSettingsUpdate, allAccessible?: boolean) => {
    setSettingsSaving(true);
    try {
      const saved = await roleAApi.saveCaptureConfig(update);
      setCaptureSettings(saved);
      if (typeof allAccessible === 'boolean') {
        const filesystem = await roleAApi.saveFilesystemCapture(allAccessible);
        setFilesystemAccessible(filesystem.all_accessible);
      }
      showStatus({ text: 'Capture settings saved locally.', variant: 'success' });
    } catch (reason) {
      throw reason instanceof Error ? reason : new Error('Could not save capture settings.');
    } finally {
      setSettingsSaving(false);
    }
  }, [showStatus]);

  const saveRetentionSettings = useCallback(async (policy: RetentionPolicy) => {
    setSettingsSaving(true);
    try {
      const saved = await roleAApi.saveRetentionPolicy(policy);
      setRetentionPolicy(saved);
      const capture = await roleAApi.captureConfig();
      setCaptureSettings(capture);
      showStatus({ text: 'Retention policy saved locally.', variant: 'success' });
    } finally {
      setSettingsSaving(false);
    }
  }, [showStatus]);

  const purgeDetailedData = useCallback(async () => {
    setSettingsSaving(true);
    try {
      await roleAApi.purgeDetailed();
      setDetailedEventCount(0);
      showStatus({ text: 'Detailed capture data deleted.', variant: 'success' });
    } finally {
      setSettingsSaving(false);
    }
  }, [showStatus]);

  const purgeRetentionData = useCallback(async (policy: RetentionPolicy) => {
    setSettingsSaving(true);
    try {
      await roleAApi.purgeRetention(policy.detailed_days ?? undefined, policy.metadata_days ?? undefined);
      showStatus({ text: 'Retention purge completed.', variant: 'success' });
    } finally {
      setSettingsSaving(false);
    }
  }, [showStatus]);

  const loadIntelligencePreview = useCallback(async () => {
    setIntelligencePreview(await roleAApi.intelligencePreviewSample());
  }, []);

  const attachGeminiCredentials = useCallback(async () => {
    setSettingsError(null);
    if (!window.intent?.pickGeminiCredentials) {
      setSettingsError('Gemini credentials can only be attached from the installed Intent desktop app.');
      return;
    }
    const result = await window.intent.pickGeminiCredentials();
    if (!result.ok) {
      if (result.error) setSettingsError(result.error);
      return;
    }
    setSettings(await roleBApi.llmSettings());
    showStatus({ text: 'Gemini service account saved locally.', variant: 'success' });
  }, [showStatus]);

  const clearGeminiCredentials = useCallback(async () => {
    setSettingsError(null);
    try {
      setSettings(await roleBApi.clearGeminiCredentials());
      showStatus({ text: 'Gemini credentials removed.', variant: 'success' });
    } catch (reason) {
      setSettingsError(reason instanceof Error ? reason.message : 'Could not remove Gemini credentials.');
    }
  }, [showStatus]);

  const completeOnboarding = useCallback((openDashboard: boolean) => {
    try {
      window.localStorage.setItem(ONBOARDING_STORAGE_KEY, 'true');
    } catch {
      // The guide is still dismissible when browser storage is unavailable.
    }
    setOnboardingStep(null);
    if (openDashboard) openOverlay();
    else closeOverlay();
  }, [closeOverlay, openOverlay]);

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
        if (onboardingVisible) return;
        if (settingsOpen) setSettingsOpen(false);
        else if (restoreReview) setRestoreReview(null);
        else closeOverlay();
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [closeOverlay, onboardingVisible, open, restoreReview, settingsOpen]);

  const requestRestoreReview = useCallback(async (intentId?: string, query?: string) => {
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
    });
  }, []);

  const continueWithTabs = useCallback(async (intentId: string) => {
    setResumeCandidates(null);
    setRestoreReview(null);
    setRestoring(true);
    try {
      const selection = await roleBApi.selectResume({ intentId });
      if (selection.needs_picker || !selection.selected) {
        showStatus({ text: 'Choose a stored session to review before continuing.', variant: 'error' });
        return;
      }
      const urls = selection.selected.resume_payload.urls;
      if (!urls.length) {
        showStatus({ text: 'No Firefox tabs were stored for this session.', variant: 'info' });
        return;
      }
      const result = await roleAApi.restore({ mode: 'continue', files: [], urls, shell: {} });
      if (result.ok) {
        showStatus({ text: `Opened ${result.restored.urls} saved Firefox tab${result.restored.urls === 1 ? '' : 's'}.`, variant: 'success' });
        closeOverlay();
      } else {
        showStatus({ text: `Could not open all saved tabs: ${result.failed.join(' ')}`, variant: 'error' });
      }
    } catch (reason) {
      showStatus({ text: reason instanceof Error ? reason.message : 'Could not open saved Firefox tabs.', variant: 'error' });
    } finally {
      setRestoring(false);
    }
  }, [closeOverlay, showStatus]);

  const confirmRestore = useCallback(async () => {
    if (!restoreReview) return;
    setRestoring(true);
    try {
      const result = await roleAApi.restore({ ...restoreReview.payload, mode: 'resume' });
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
      if (settingsOpen) setSettingsOpen(false);
      else closeOverlay();
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
    void requestRestoreReview(selectedId ?? undefined, parsed.mode === 'restore' ? parsed.query : undefined);
  };

  const inputBusy = loading || content.kind === 'search' && content.loading || content.kind === 'copilot' && content.loading;
  const overlayVisible = open || onboardingVisible || settingsOpen;
  return (
    <main className={`overlay-shell ${overlayVisible ? 'is-open' : 'is-idle'}`}>
      {onboardingVisible ? (
        <WelcomeScreen
          step={onboardingStep ?? 0}
          onBack={() => setOnboardingStep((step) => Math.max(0, (step ?? 0) - 1))}
          onNext={() => setOnboardingStep((step) => Math.min(2, (step ?? 0) + 1))}
          onFinish={() => completeOnboarding(true)}
          onSkip={() => completeOnboarding(false)}
          onOpenSettings={settingsAvailable ? () => {
            completeOnboarding(false);
            void openSettings('intelligence');
          } : undefined}
        />
      ) : (
        <div className={`command-region ${restoreReview ? 'has-restore-review' : ''}`}>
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
            productionSettings={settingsAvailable}
            onOpenSettings={() => void openSettings()}
          />
          {open && (settingsOpen ? (
            <SettingsPanel
              activeTab={settingsTab}
              onTabChange={setSettingsTab}
              settings={settings}
              capture={captureSettings}
              retention={retentionPolicy}
              filesystemAccessible={filesystemAccessible}
              detailedEventCount={detailedEventCount}
              preview={intelligencePreview}
              loading={settingsLoading}
              saving={settingsSaving}
              error={settingsError}
              electronAvailable={productionSettingsAvailable}
              onClose={() => setSettingsOpen(false)}
              onSaveLlm={(update) => void saveSettings(update)}
              onSaveCapture={saveCaptureSettings}
              onSaveRetention={saveRetentionSettings}
              onPurgeDetailed={purgeDetailedData}
              onPurgeRetention={purgeRetentionData}
              onLoadPreview={loadIntelligencePreview}
              onAttachGeminiCredentials={attachGeminiCredentials}
              onClearGeminiCredentials={clearGeminiCredentials}
            />
          ) : (
            <div className={`dashboard-layout ${restoreReview ? 'has-restore-review' : ''}`}>
              <SessionDashboard
                data={data}
                content={content}
                loading={loading}
                error={error}
                selectedId={selectedId}
                resumeCandidates={resumeCandidates}
                onRetry={() => void load().catch(() => undefined)}
                onSelect={setSelectedId}
                onReview={(intentId) => void requestRestoreReview(intentId)}
                onContinue={(intentId) => void continueWithTabs(intentId)}
                onCopilotResume={reviewCopilotProposal}
              />
              {restoreReview && (
                <RestoreReview
                  review={restoreReview}
                  restoring={restoring}
                  onClose={() => setRestoreReview(null)}
                  onConfirm={() => void confirmRestore()}
                />
              )}
            </div>
          ))}
        </div>
      )}
      <StatusToast status={status} />
    </main>
  );
}
