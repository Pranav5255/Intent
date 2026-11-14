import { useCallback, useEffect, useState } from 'react';
import { Icon, type IconName } from '../components/Icon';
import { RestoreReview, type ReviewTarget } from '../components/RestoreReview';
import { Keys, StatusDot, Toasts } from '../components/ui';
import { api } from '../lib/api';
import { listen, takePendingRoute } from '../lib/bridge';
import { modHeld } from '../lib/env';
import { clock } from '../lib/format';
import type { Intent } from '../lib/types';
import {
  errorText,
  pausedUntilLabel,
  refreshCapture,
  resumeCapture,
  summarizeCapture,
  toast,
  useAsync,
  useCapture,
} from '../lib/store';
import { SettingsView } from '../settings/SettingsView';
import { IntentDrawer } from './IntentDrawer';
import { Onboarding, onboardingDone } from './Onboarding';
import { SearchView } from './SearchView';
import { TimelineView } from './TimelineView';
import { TodayView } from './TodayView';

export type Route = 'today' | 'timeline' | 'search' | 'settings';

const NAV: Array<{ id: Route; label: string; icon: IconName; key: string }> = [
  { id: 'today', label: 'Today', icon: 'today', key: '1' },
  { id: 'timeline', label: 'Timeline', icon: 'timeline', key: '2' },
  { id: 'search', label: 'Search', icon: 'search', key: '3' },
  { id: 'settings', label: 'Settings', icon: 'settings', key: ',' },
];

function parseHash(): { route: Route; intent: string | null; section: string | null } {
  const h = window.location.hash.replace(/^#\/main\/?/, '');
  if (h.startsWith('intent:')) return { route: 'today', intent: h.slice(7), section: null };
  if (h.startsWith('settings:')) return { route: 'settings', intent: null, section: h.slice(9) };
  const route = (['today', 'timeline', 'search', 'settings'] as Route[]).find((r) => r === h) ?? 'today';
  return { route, intent: null, section: null };
}

export interface MainActions {
  openIntent: (id: string) => void;
  review: (intent: Intent, only?: 'tabs') => void;
}

export function MainWindow() {
  const initial = parseHash();
  const [route, setRoute] = useState<Route>(initial.route);
  const [settingsSection, setSettingsSection] = useState<string | null>(initial.section);
  const [drawer, setDrawer] = useState<string | null>(initial.intent);
  const [review, setReview] = useState<ReviewTarget | null>(null);
  const [onboarding, setOnboarding] = useState(() => !onboardingDone());
  const capture = useCapture();
  const summary = summarizeCapture(capture);
  const current = useAsync(() => api.current(), [capture.status?.capture_paused]);

  const navigate = useCallback((target: string) => {
    if (target.startsWith('intent:')) {
      setDrawer(target.slice(7));
      return;
    }
    if (target.startsWith('settings:')) {
      setRoute('settings');
      setSettingsSection(target.slice(9));
      return;
    }
    if (['today', 'timeline', 'search', 'settings'].includes(target)) setRoute(target as Route);
  }, []);

  // Rust emits `navigate` for tray items, `open_main(route)` and `--preview <id>`.
  useEffect(() => {
    // A route requested before this webview finished loading (cold start, --preview).
    void takePendingRoute().then((route) => route && navigate(route));
    const un = listen<string>('navigate', navigate);
    const onHash = () => {
      const h = window.location.hash.replace(/^#\/main\/?/, '');
      if (h) navigate(h);
    };
    window.addEventListener('hashchange', onHash);
    return () => {
      void un.then((fn) => fn());
      window.removeEventListener('hashchange', onHash);
    };
  }, [navigate]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!modHeld(e) || e.altKey) return;
      const item = NAV.find((n) => n.key === e.key);
      if (item) {
        e.preventDefault();
        setRoute(item.id);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const actions: MainActions = {
    openIntent: (id) => setDrawer(id),
    review: (intent, only) =>
      setReview({
        id: intent.id,
        label: intent.label,
        summary: intent.summary,
        tags: intent.tags,
        end_ts: intent.end_ts,
        duration_seconds: intent.stats.duration_seconds,
        payload: intent.resume_payload,
        insights: intent.insights,
        only,
      }),
  };

  const status = capture.status;
  const paused = Boolean(status?.capture_paused);

  return (
    <div className="mw">
      <nav className="rail" aria-label="Sections">
        <div className="rail-brand">
          <BrandMark />
          <span className="rail-name display">Intent</span>
        </div>
        <div className="rail-nav">
          {NAV.map((n) => (
            <button
              key={n.id}
              className={`rail-item ${route === n.id ? 'is-active' : ''}`}
              aria-current={route === n.id ? 'page' : undefined}
              onClick={() => setRoute(n.id)}
            >
              <Icon name={n.icon} size={15} />
              <span>{n.label}</span>
              <span className="rail-key">
                <Keys keys={['mod', n.key]} />
              </span>
            </button>
          ))}
        </div>
        <div className="rail-foot">
          {current.data && !paused && (
            <button className="rail-now" onClick={() => setRoute('today')} title={current.data.summary}>
              <span className="rail-now-label">Now</span>
              <span className="rail-now-text ellipsis">{current.data.label}</span>
              <span className="mono t3">since {clock(current.data.since_ts)}</span>
            </button>
          )}
          <button className={`rail-status is-${summary.tone}`} onClick={() => navigate('settings:capture')}>
            <StatusDot tone={summary.tone} />
            <span className="ellipsis">{summary.text}</span>
          </button>
        </div>
      </nav>

      <div className="mw-main">
        {paused && (
          <div className="banner" role="status">
            <span className="dot" />
            <span>Capture paused</span>
            {status?.capture_paused_until && (
              <span className="banner-meta">· resumes {pausedUntilLabel(status.capture_paused_until)}</span>
            )}
            <button
              className="btn btn-sm"
              onClick={() =>
                void resumeCapture()
                  .then(() => toast({ tone: 'success', text: 'Capture resumed' }))
                  .catch((r: unknown) => toast({ tone: 'error', text: 'Couldn’t resume capture', detail: errorText(r) }))
              }
            >
              Resume now
            </button>
          </div>
        )}
        {capture.reachable === false && (
          <div className="banner is-offline" role="status">
            <span className="dot" />
            <span>Intent isn’t running</span>
            <span className="banner-meta">· nothing is being captured</span>
            <button className="btn btn-sm" onClick={() => void refreshCapture()}>
              Try again
            </button>
          </div>
        )}
        <div className="mw-content">
          {route === 'today' && <TodayView actions={actions} />}
          {route === 'timeline' && <TimelineView actions={actions} />}
          {route === 'search' && <SearchView actions={actions} />}
          {route === 'settings' && <SettingsView section={settingsSection} onSectionShown={() => setSettingsSection(null)} />}
        </div>
      </div>

      {drawer && <IntentDrawer id={drawer} actions={actions} onClose={() => setDrawer(null)} />}
      {review && (
        <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && setReview(null)}>
          <RestoreReview variant="sheet" target={review} onCancel={() => setReview(null)} onDone={() => setReview(null)} />
        </div>
      )}
      {onboarding && <Onboarding onDone={() => setOnboarding(false)} />}
      <Toasts />
    </div>
  );
}

export function BrandMark({ size = 20 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 20 20" aria-hidden="true">
      <rect width="20" height="20" rx="5" fill="var(--raised)" />
      <rect x="0.5" y="0.5" width="19" height="19" rx="4.5" fill="none" stroke="var(--border-strong)" />
      <path d="M5.5 12.6 8.4 9.7l2.2 2.2 3.9-3.9" fill="none" stroke="var(--accent)" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}
