import { useEffect, useRef, useState } from 'react';
import { StatusDot } from '../components/ui';
import { api } from '../lib/api';
import { IS_MAC, MOCK, START_DAEMON } from '../lib/env';
import { ago } from '../lib/format';
import { BROWSER_SOURCES, combinedStatus } from '../lib/sources';
import { errorText, refreshCapture, toast, useAsync, useCapture, useRetryOnReconnect } from '../lib/store';
import { BrandMark } from './MainWindow';

const KEY = 'intent.onboarded.v2';

export function onboardingDone(): boolean {
  const firstRun = new URLSearchParams(window.location.search).get('firstrun') === '1';
  if (firstRun) return false;
  if (MOCK) return true;
  try {
    return window.localStorage.getItem(KEY) === '1';
  } catch {
    return true;
  }
}

function markDone() {
  try {
    window.localStorage.setItem(KEY, '1');
  } catch {
    // Not fatal: the sheet just shows again next launch.
  }
}

const COMPANIONS: Array<{ sources: string[]; name: string; missing: string }> = [
  {
    sources: ['linux'],
    name: 'Desktop activity',
    missing: IS_MAC ? 'Starts with “intentctl enable”' : 'Needs the GNOME extension on Wayland',
  },
  { sources: ['vscode'], name: 'VS Code companion', missing: 'Install “Intent” from the VS Code marketplace' },
  { sources: BROWSER_SOURCES, name: 'Browser companion', missing: 'Install the Intent extension for Firefox or Chrome' },
  { sources: ['shell'], name: 'Shell history', missing: 'Run “intentctl shell enable” once in your shell' },
];

/** First run: one sentence, live checks, one button. No carousel. */
export function Onboarding({ onDone }: { onDone: () => void }) {
  const capture = useCapture();
  const health = useAsync(() => api.health(), []);
  // The capture poll notices when the service starts; check health again then.
  useRetryOnReconnect(health);
  const reachable = health.data ? true : health.error ? false : null;
  const [busy, setBusy] = useState(false);
  const primary = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    primary.current?.focus();
  }, []);

  const start = async () => {
    setBusy(true);
    try {
      if (capture.status?.capture_paused) await api.pause(false);
      await refreshCapture();
      markDone();
      onDone();
    } catch (reason) {
      toast({ tone: 'error', text: 'Couldn’t start capturing', detail: errorText(reason) });
    } finally {
      setBusy(false);
    }
  };

  const sources = capture.status?.sources ?? {};

  return (
    <div className="scrim">
      <div className="onb" role="dialog" aria-labelledby="onb-title">
        <div className="onb-head">
          <BrandMark size={28} />
          <h2 id="onb-title" className="onb-title display">
            Intent remembers what you were working on
          </h2>
          <p className="onb-text t2">
            It records which files, tabs and commands belong together, keeps it on this computer, and lets you pick any
            thread back up with one key.
          </p>
        </div>
        <div className="onb-checks boxed">
          <div className="onb-check">
            <StatusDot tone={reachable === null ? 'idle' : reachable ? 'capturing' : 'offline'} />
            <span className="onb-check-name">Intent service</span>
            <span className="onb-check-state t2">
              {reachable === null ? 'Checking…' : reachable ? 'Running' : `Not running — start it with “${START_DAEMON}”`}
            </span>
          </div>
          {COMPANIONS.map((c) => {
            const s = combinedStatus(sources, c.sources);
            const seen = s?.last_event_ts ?? null;
            return (
              <div key={c.name} className="onb-check">
                <StatusDot tone={s?.healthy ? 'capturing' : 'idle'} />
                <span className="onb-check-name">{c.name}</span>
                <span className="onb-check-state t2">{seen ? `Seen ${ago(seen)}` : c.missing}</span>
              </div>
            );
          })}
        </div>
        <div className="onb-foot">
          <span className="t3 onb-note">You can pause capture any time from the tray.</span>
          <button
            className="btn btn-quiet"
            onClick={() => {
              markDone();
              onDone();
            }}
          >
            Not now
          </button>
          <button ref={primary} className="btn btn-primary" disabled={busy || reachable === false} onClick={() => void start()}>
            Start capturing
          </button>
        </div>
      </div>
    </div>
  );
}
