import { useEffect, useState } from 'react';
import { Icon } from '../components/Icon';
import { ConfirmDialog, Keys, StatusDot, Switch, type ConfirmSpec } from '../components/ui';
import { api } from '../lib/api';
import { appVersion, getAutostart, getShortcut, setAutostart, setShortcut } from '../lib/bridge';
import { IS_MAC } from '../lib/env';
import { ago, bytes, count, tildify } from '../lib/format';
import { BROWSER_SOURCES, combinedStatus } from '../lib/sources';
import {
  errorText,
  intentsChanged,
  pauseCapture,
  pausedUntilLabel,
  refreshCapture,
  resumeCapture,
  toast,
  useAsync,
  useCapture,
  useRetryOnReconnect,
  type PauseSpan,
} from '../lib/store';
import { saveTheme, storedTheme, type Theme } from '../lib/theme';
import type { DetailedCaptureConfig, RetentionPolicy } from '../lib/types';
import { Group, Row, Section } from './kit';
import { ShortcutsSheet } from './Shortcuts';

// ---------------------------------------------------------------------------
// General
// ---------------------------------------------------------------------------

function acceleratorFrom(e: KeyboardEvent): string | null {
  const key = e.key;
  if (['Control', 'Shift', 'Alt', 'Meta', 'Super', 'OS'].includes(key)) return null;
  const mods: string[] = [];
  if (e.metaKey) mods.push('Super');
  if (e.ctrlKey) mods.push('Ctrl');
  if (e.altKey) mods.push('Alt');
  if (e.shiftKey) mods.push('Shift');
  if (!mods.length) return null;
  const name = key === ' ' ? 'Space' : key.length === 1 ? key.toUpperCase() : key;
  return [...mods, name].join('+');
}

function AcceleratorKeys({ accel }: { accel: string }) {
  const glyph = (k: string) => (k === 'Super' ? (IS_MAC ? '⌘' : 'Super') : k === 'Ctrl' ? (IS_MAC ? '⌃' : 'Ctrl') : k);
  return <Keys keys={accel.split('+').map(glyph)} />;
}

export function GeneralSection() {
  const [shortcut, setShortcutState] = useState<string>('Super+Space');
  const [recording, setRecording] = useState(false);
  const [autostart, setAutostartState] = useState<boolean | null>(null);
  const [theme, setTheme] = useState<Theme>(storedTheme());

  useEffect(() => {
    void getShortcut().then(setShortcutState);
    void getAutostart().then(setAutostartState);
  }, []);

  useEffect(() => {
    if (!recording) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === 'Escape') {
        setRecording(false);
        return;
      }
      const accel = acceleratorFrom(e);
      if (!accel) return;
      setRecording(false);
      setShortcut(accel)
        .then(() => {
          setShortcutState(accel);
          toast({ tone: 'success', text: `Intent now opens with ${accel.replace(/\+/g, ' + ')}` });
        })
        .catch((r: unknown) => toast({ tone: 'error', text: 'That shortcut is taken', detail: errorText(r) }));
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [recording]);

  return (
    <Section id="general" title="General" icon="sliders">
      <Group>
        <Row title="Open Intent" subtitle="Shows the palette from anywhere" keywords="shortcut hotkey keyboard global overlay">
          {recording ? (
            <span className="recording mono">Press keys… <Keys keys={['Esc']} /></span>
          ) : (
            <AcceleratorKeys accel={shortcut} />
          )}
          <button className="btn btn-sm" onClick={() => setRecording((v) => !v)}>
            {recording ? 'Cancel' : 'Change'}
          </button>
        </Row>
        <Row title="Launch at login" subtitle="Start in the tray when you sign in" keywords="autostart startup boot login">
          <Switch
            label="Launch at login"
            checked={Boolean(autostart)}
            disabled={autostart === null}
            onChange={(v) => {
              setAutostartState(v);
              setAutostart(v).catch((r: unknown) => {
                setAutostartState(!v);
                toast({ tone: 'error', text: 'Couldn’t change login item', detail: errorText(r) });
              });
            }}
          />
        </Row>
        <Row title="Appearance" keywords="theme dark light mode colour color appearance">
          <div className="segmented" role="group" aria-label="Appearance">
            {(['system', 'light', 'dark'] as Theme[]).map((t) => (
              <button
                key={t}
                aria-pressed={theme === t}
                onClick={() => {
                  setTheme(t);
                  saveTheme(t);
                }}
              >
                {t === 'system' ? 'System' : t === 'light' ? 'Light' : 'Dark'}
              </button>
            ))}
          </div>
        </Row>
      </Group>
    </Section>
  );
}

// ---------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------

type BrowserLevel = 'metadata' | 'actions' | 'excerpts';

function browserLevel(c: DetailedCaptureConfig | null): BrowserLevel {
  if (!c?.browser.enabled) return 'metadata';
  return c.browser.context_enabled ? 'excerpts' : 'actions';
}

const SOURCES: Array<{ key: string; name: string; what: string; sources?: string[] }> = [
  { key: 'vscode', name: 'Editor', what: 'VS Code files and edits' },
  { key: 'firefox', name: 'Browser', what: 'Firefox and Chrome tabs', sources: BROWSER_SOURCES },
  { key: 'shell', name: 'Terminal', what: 'Commands and folders' },
  { key: 'linux', name: 'Desktop', what: 'Focused windows and idle time' },
  { key: 'filesystem', name: 'Files', what: 'Changes in approved folders' },
];

export function CaptureSection() {
  const capture = useCapture();
  const config = useAsync(() => api.captureConfig(), []);
  const workspaces = useAsync(() => api.workspaces(), []);
  const fs = useAsync(() => api.filesystemCapture(), []);
  useRetryOnReconnect(config, workspaces, fs);
  const [newPath, setNewPath] = useState('');
  const status = capture.status;
  const paused = Boolean(status?.capture_paused);

  const saveConfig = async (update: Parameters<typeof api.saveCaptureConfig>[0], ok: string) => {
    try {
      const saved = await api.saveCaptureConfig(update);
      config.setData(() => saved);
      toast({ tone: 'success', text: ok });
    } catch (r) {
      toast({ tone: 'error', text: 'Couldn’t save capture settings', detail: errorText(r) });
    }
  };

  const saveWorkspaces = async (list: string[]) => {
    try {
      const saved = await api.saveWorkspaces(list);
      workspaces.setData(() => saved);
      void refreshCapture();
    } catch (r) {
      toast({ tone: 'error', text: 'Couldn’t update folders', detail: errorText(r) });
    }
  };

  const sourceControl = (key: string) => {
    const c = config.data;
    if (!c) return null;
    if (key === 'vscode') {
      return (
        <Switch
          label="Record edits"
          checked={c.editor.enabled}
          onChange={(v) => void saveConfig({ editor: { enabled: v } }, v ? 'Editor edits will be recorded' : 'Editor edits are off')}
        />
      );
    }
    if (key === 'firefox') {
      return (
        <select
          className="field field-sm"
          aria-label="Browser detail"
          value={browserLevel(c)}
          onChange={(e) => {
            const level = e.target.value as BrowserLevel;
            void saveConfig(
              { browser: { enabled: level !== 'metadata', context_enabled: level === 'excerpts' } },
              'Browser detail updated',
            );
          }}
        >
          <option value="metadata">Titles and addresses</option>
          <option value="actions">+ clicks and scrolling</option>
          <option value="excerpts">+ short page excerpts</option>
        </select>
      );
    }
    if (key === 'filesystem') {
      return (
        <Switch
          label="Watch approved folders"
          checked={c.filesystem.enabled}
          onChange={(v) => void saveConfig({ filesystem: { enabled: v } }, v ? 'Watching approved folders' : 'Folder watching is off')}
        />
      );
    }
    return null;
  };

  return (
    <Section id="capture" title="Capture" icon="capture">
      <Group
        title="Sources"
        description={
          status
            ? `Live from this computer${status.session_type !== 'unknown' ? ` · ${status.session_type === 'wayland' ? 'Wayland' : 'X11'} session` : ''}`
            : 'Checking sources…'
        }
      >
        {SOURCES.map((s) => {
          const st = combinedStatus(status?.sources, s.sources ?? [s.key]);
          const seen = st?.last_event_ts ?? null;
          return (
            <Row
              key={s.key}
              lead={<StatusDot tone={paused ? 'paused' : st?.healthy ? 'capturing' : 'idle'} />}
              title={
                <>
                  {s.name} <span className="t3">· {s.what}</span>
                </>
              }
              keywords={`${s.name} ${s.what} source ${s.key} status`}
              subtitle={
                <span className="mono">
                  {seen ? `last event ${ago(seen)}` : 'no events yet'}
                  {st ? ` · ${count(st.event_count)} events` : ''}
                </span>
              }
            >
              {sourceControl(s.key)}
            </Row>
          );
        })}
      </Group>

      <Group title="Pause">
        <Row
          title={paused ? 'Capture is paused' : 'Pause capture'}
          subtitle={
            paused
              ? status?.capture_paused_until
                ? `Resumes on its own at ${pausedUntilLabel(status.capture_paused_until)}`
                : 'Until you resume it'
              : 'Nothing is recorded from any source while paused'
          }
          keywords="pause stop privacy break resume"
        >
          {paused ? (
            <button className="btn btn-sm" onClick={() => void resumeCapture()}>
              Resume now
            </button>
          ) : (
            <select
              className="field field-sm"
              aria-label="Pause capture for"
              value=""
              onChange={(e) => {
                const span = e.target.value as PauseSpan;
                if (span)
                  void pauseCapture(span)
                    .then((until) => toast({ tone: 'neutral', text: `Capture paused until ${pausedUntilLabel(until)}` }))
                    .catch((r: unknown) => toast({ tone: 'error', text: 'Couldn’t pause', detail: errorText(r) }));
              }}
            >
              <option value="" disabled>
                Pause for…
              </option>
              <option value="15m">15 minutes</option>
              <option value="1h">1 hour</option>
              <option value="tomorrow">Until tomorrow</option>
            </select>
          )}
        </Row>
      </Group>

      <Group
        title="Approved folders"
        description="Editor edits and file changes are only recorded inside these folders."
      >
        {(workspaces.data?.workspaces ?? []).map((p) => (
          <Row key={p} icon="folder" title={<span className="mono">{tildify(p)}</span>} keywords={`workspace folder approved ${p}`}>
            <button
              className="btn btn-quiet btn-sm"
              aria-label={`Remove ${p}`}
              onClick={() => void saveWorkspaces((workspaces.data?.workspaces ?? []).filter((x) => x !== p))}
            >
              Remove
            </button>
          </Row>
        ))}
        <Row title="Add a folder" keywords="workspace folder approved add">
          <form
            className="inline-form"
            onSubmit={(e) => {
              e.preventDefault();
              const path = newPath.trim();
              if (!path) return;
              void saveWorkspaces([...(workspaces.data?.workspaces ?? []), path]).then(() => setNewPath(''));
            }}
          >
            <input
              className="field field-sm mono"
              placeholder="~/code/project"
              value={newPath}
              onChange={(e) => setNewPath(e.target.value)}
              aria-label="Folder path"
            />
            <button className="btn btn-sm" type="submit" disabled={!newPath.trim()}>
              <Icon name="plus" size={13} /> Add
            </button>
          </form>
        </Row>
        <Row
          title="Watch every folder I can open"
          subtitle="Records file changes anywhere in your home folder, not just approved folders"
          keywords="filesystem all accessible files broad"
        >
          <Switch
            label="Watch every folder"
            checked={Boolean(fs.data?.all_accessible)}
            disabled={!fs.data}
            onChange={(v) =>
              void api
                .saveFilesystemCapture(v)
                .then((saved) => fs.setData(() => saved))
                .catch((r: unknown) => toast({ tone: 'error', text: 'Couldn’t change file watching', detail: errorText(r) }))
            }
          />
        </Row>
      </Group>
    </Section>
  );
}

// ---------------------------------------------------------------------------
// Privacy
// ---------------------------------------------------------------------------

const PRESETS: Array<{ id: string; label: string; policy: RetentionPolicy }> = [
  { id: 'forever', label: 'Keep until I delete it', policy: { metadata_days: null, detailed_days: null } },
  { id: '30-7', label: '30 days, details for 7', policy: { metadata_days: 30, detailed_days: 7 } },
  { id: '90-30', label: '90 days, details for 30', policy: { metadata_days: 90, detailed_days: 30 } },
  { id: '365-90', label: 'A year, details for 90 days', policy: { metadata_days: 365, detailed_days: 90 } },
];

function presetFor(p: RetentionPolicy | null): string {
  if (!p) return '';
  return PRESETS.find((x) => x.policy.metadata_days === p.metadata_days && x.policy.detailed_days === p.detailed_days)?.id ?? 'custom';
}

export function PrivacySection() {
  const capture = useCapture();
  const blocked = useAsync(() => api.blockedDomains(), []);
  const policy = useAsync(() => api.retentionPolicy(), []);
  const config = useAsync(() => api.captureConfig(), []);
  useRetryOnReconnect(blocked, policy, config);
  const [confirm, setConfirm] = useState<ConfirmSpec | null>(null);
  // Initialise from the SAVED policy (1.x always showed the first preset).
  const current = presetFor(policy.data);
  const detailedCount = Object.values(capture.status?.detailed_capture?.event_counts ?? {}).reduce((a, b) => a + b, 0);

  const savePolicy = async (id: string) => {
    const preset = PRESETS.find((p) => p.id === id);
    if (!preset) return;
    try {
      const saved = await api.saveRetentionPolicy(preset.policy);
      policy.setData(() => saved);
      toast({ tone: 'success', text: 'Retention saved', detail: 'Older data is removed at the next daily cleanup.' });
    } catch (r) {
      toast({ tone: 'error', text: 'Couldn’t save retention', detail: errorText(r) });
    }
  };

  const confirmPurge = async () => {
    const p = policy.data;
    if (!p || (p.metadata_days === null && p.detailed_days === null)) {
      toast({ tone: 'neutral', text: 'Nothing to clean up', detail: 'Retention is set to keep everything.' });
      return;
    }
    try {
      const preview = await api.retentionPreview(p);
      setConfirm({
        title: 'Run retention cleanup now?',
        text: 'Events and threads older than your retention period are deleted from this computer.',
        facts: [
          ['Detailed events', count(preview.eligible.detailed)],
          ['Other events', count(preview.eligible.metadata)],
          ['Total events to delete', count(preview.eligible.total)],
          ['Threads to delete', count(preview.eligible.intents)],
        ],
        confirm: `Delete ${count(preview.eligible.total)} events`,
        destructive: true,
        onConfirm: async () => {
          try {
            await api.purgeRetention(p);
            intentsChanged();
            toast({ tone: 'success', text: `Deleted ${count(preview.eligible.total)} old events` });
            void refreshCapture();
          } catch (r) {
            toast({ tone: 'error', text: 'Cleanup failed', detail: errorText(r) });
          }
        },
      });
    } catch (r) {
      toast({ tone: 'error', text: 'Couldn’t check what would be deleted', detail: errorText(r) });
    }
  };

  const confirmDetailed = async () => {
    let olderThanPolicy: number | null = null;
    if (policy.data?.detailed_days) {
      olderThanPolicy = await api
        .retentionPreview({ detailed_days: policy.data.detailed_days, metadata_days: null })
        .then((r) => r.eligible.detailed)
        .catch(() => null);
    }
    setConfirm({
      title: 'Delete all detailed data?',
      text: 'Edits, page excerpts and file contents are deleted. Titles, addresses and your threads are kept.',
      facts: [
        ['Detailed events stored', count(detailedCount)],
        ...(olderThanPolicy !== null ? ([['Of these, past retention', count(olderThanPolicy)]] as Array<[string, string]>) : []),
      ],
      confirm: 'Delete detailed data',
      destructive: true,
      onConfirm: async () => {
        try {
          const r = await api.purgeDetailed();
          toast({ tone: 'success', text: `Deleted ${count(r.deleted)} detailed events` });
          void refreshCapture();
        } catch (r) {
          toast({ tone: 'error', text: 'Couldn’t delete detailed data', detail: errorText(r) });
        }
      },
    });
  };

  return (
    <Section id="privacy" title="Privacy" icon="shield">
      <Group title="Never recorded" description="Pages on these sites and files matching these patterns are dropped before they are stored.">
        <Row
          title="Blocked sites"
          subtitle={<span className="mono">{(blocked.data?.blocked_domains ?? []).join('  ·  ') || '—'}</span>}
          keywords="blocked domains sites blocklist never record"
        >
          <span className="t3 num">{blocked.data?.blocked_domains.length ?? 0}</span>
        </Row>
        <Row
          title="Ignored files"
          subtitle={<span className="mono">{(config.data?.editor.excluded_patterns ?? []).join('  ·  ') || '—'}</span>}
          keywords="excluded patterns files secrets env ignore"
        >
          <span className="t3 num">{config.data?.editor.excluded_patterns.length ?? 0}</span>
        </Row>
      </Group>

      <Group title="Retention" description="Details such as edits and page excerpts can expire sooner than the rest.">
        <Row title="Keep activity for" keywords="retention keep days delete expire history">
          <select
            className="field field-sm"
            aria-label="Retention"
            value={current}
            disabled={!policy.data}
            onChange={(e) => void savePolicy(e.target.value)}
          >
            {current === 'custom' && policy.data && (
              <option value="custom">
                Custom: {policy.data.metadata_days ?? '∞'} days, details {policy.data.detailed_days ?? '∞'}
              </option>
            )}
            {PRESETS.map((p) => (
              <option key={p.id} value={p.id}>
                {p.label}
              </option>
            ))}
          </select>
        </Row>
        <Row title="Run cleanup now" subtitle="Applies the retention period immediately instead of waiting for the daily run" keywords="purge retention cleanup delete old">
          <button className="btn btn-sm" onClick={() => void confirmPurge()}>
            Review…
          </button>
        </Row>
        <Row
          title="Delete detailed data"
          subtitle={`${count(detailedCount)} detailed events stored`}
          keywords="delete purge detailed data edits excerpts"
          danger
        >
          <button className="btn btn-danger btn-sm" onClick={() => void confirmDetailed()}>
            Delete…
          </button>
        </Row>
      </Group>
      {confirm && <ConfirmDialog spec={confirm} onClose={() => setConfirm(null)} />}
    </Section>
  );
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

export function StorageSection() {
  const capture = useCapture();
  const s = capture.status;
  const totals = Object.entries(s?.sources ?? {}).sort((a, b) => b[1].event_count - a[1].event_count);
  const total = totals.reduce((sum, [, v]) => sum + v.event_count, 0);
  const detailed = Object.values(s?.detailed_capture?.event_counts ?? {}).reduce((a, b) => a + b, 0);
  const names: Record<string, string> = { vscode: 'Editor', firefox: 'Firefox', chrome: 'Chrome', shell: 'Terminal', linux: 'Desktop', filesystem: 'Files' };
  return (
    <Section id="storage" title="Storage" icon="database">
      <Group>
        <Row title="Database" subtitle={<span className="mono">~/.local/share/intent/intent.db</span>} keywords="database size disk storage">
          <span className="num stat-inline">{s?.db_size_bytes ? bytes(s.db_size_bytes) : '—'}</span>
        </Row>
        <Row title="Events stored" keywords="events count total storage">
          <span className="num stat-inline">{count(total)}</span>
        </Row>
        <Row title="Detailed events" subtitle="Edits, page excerpts and file contents" keywords="detailed events count">
          <span className="num stat-inline">{count(detailed)}</span>
        </Row>
      </Group>
      <Group title="By source">
        {totals.map(([k, v]) => (
          <Row key={k} title={names[k] ?? k} keywords={`source events ${k}`}>
            <span className="meter" aria-hidden="true">
              <span style={{ width: `${total ? (v.event_count / total) * 100 : 0}%` }} />
            </span>
            <span className="num stat-inline">{count(v.event_count)}</span>
          </Row>
        ))}
      </Group>
    </Section>
  );
}

// ---------------------------------------------------------------------------
// Shortcuts, About
// ---------------------------------------------------------------------------

export function ShortcutsSection() {
  const [open, setOpen] = useState(false);
  return (
    <Section id="shortcuts" title="Shortcuts" icon="keyboard">
      <Group>
        <Row title="Keyboard shortcuts" subtitle="Everything in Intent works from the keyboard" keywords="keyboard shortcuts keys help">
          <button className="btn btn-sm" onClick={() => setOpen(true)}>
            Show all
          </button>
        </Row>
      </Group>
      {open && <ShortcutsSheet onClose={() => setOpen(false)} />}
    </Section>
  );
}

export function AboutSection() {
  const health = useAsync(() => api.health(), []);
  useRetryOnReconnect(health);
  const [version, setVersion] = useState('');
  useEffect(() => {
    void appVersion().then(setVersion);
  }, []);
  return (
    <Section id="about" title="About" icon="info">
      <Group>
        <Row title="Intent" subtitle="Remembers what you were working on. Everything stays on this computer unless you choose a cloud model." keywords="about version">
          <span className="mono t2">{version}</span>
        </Row>
        <Row title="Background service" keywords="service daemon version running">
          <StatusDot tone={health.data ? 'capturing' : health.error ? 'offline' : 'idle'} />
          <span className="mono t2">{health.data ? `running · ${health.data.version ?? ''}` : health.error ? 'not running' : 'checking'}</span>
        </Row>
        <Row title="Source code" subtitle={<span className="mono">github.com/Pranav5255/Intent</span>} keywords="source github license mit">
          <span className="t3">MIT</span>
        </Row>
      </Group>
    </Section>
  );
}
