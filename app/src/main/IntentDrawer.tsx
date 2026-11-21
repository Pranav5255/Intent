import { useEffect, useState } from 'react';
import { Icon } from '../components/Icon';
import { RestoreChecklist } from '../components/RestoreChecklist';
import { ConfirmDialog, Empty, Keys, Project, type ConfirmSpec } from '../components/ui';
import { api } from '../lib/api';
import { copyText } from '../lib/bridge';
import { basename, count, duration, range, relativeDay, tildify } from '../lib/format';
import { laneFor, projectOf } from '../lib/intent';
import { describeSelection, restoreSelection, selectAll, selectionSize, type RestoreSelection } from '../lib/restore';
import { errorText, intentsChanged, toast, useAsync } from '../lib/store';
import type { Intent } from '../lib/types';
import { LeftOffLine } from '../overlay/PreviewPane';
import type { MainActions } from './MainWindow';

const SOURCE_NAMES: Record<string, string> = {
  vscode: 'Editor',
  firefox: 'Browser',
  chrome: 'Browser',
  shell: 'Terminal',
  linux: 'Desktop',
  filesystem: 'Files',
};

export function IntentDrawer({ id, actions, onClose }: { id: string; actions: MainActions; onClose: () => void }) {
  const [stack, setStack] = useState<string[]>([id]);
  const current = stack[stack.length - 1]!;
  const data = useAsync(() => api.intent(current), [current]);
  const [selection, setSelection] = useState<RestoreSelection | null>(null);
  const [confirm, setConfirm] = useState<ConfirmSpec | null>(null);

  useEffect(() => setStack([id]), [id]);
  useEffect(() => {
    if (data.data) setSelection(selectAll(data.data.resume_payload));
  }, [data.data]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !confirm) {
        e.preventDefault();
        if (stack.length > 1) setStack((s) => s.slice(0, -1));
        else onClose();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose, stack.length, confirm]);

  const intent = data.data;

  return (
    <div className="drawer-scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <aside className="drawer" role="dialog" aria-label={intent?.label ?? 'Thread details'}>
        <header className="drawer-bar">
          {stack.length > 1 ? (
            <button className="btn btn-quiet btn-sm" onClick={() => setStack((s) => s.slice(0, -1))}>
              <Icon name="chevron-left" size={14} /> Back
            </button>
          ) : (
            <span className="t3 drawer-crumb">Thread</span>
          )}
          <span className="drawer-bar-actions">
            {intent && (
              <button
                className="btn btn-quiet btn-sm"
                onClick={() =>
                  void api
                    .context(intent.id)
                    .then((c) => copyText(c.markdown))
                    .then(() => toast({ tone: 'success', text: 'Context copied as Markdown' }))
                    .catch((r: unknown) => toast({ tone: 'error', text: 'Couldn’t copy context', detail: errorText(r) }))
                }
              >
                <Icon name="markdown" size={14} /> Copy context
              </button>
            )}
            <button className="btn btn-quiet btn-icon btn-sm" onClick={onClose} aria-label="Close">
              <Icon name="x" size={14} />
            </button>
          </span>
        </header>

        {!intent ? (
          data.error ? (
            <Empty icon="alert" title="Couldn’t load this thread" text={data.error.message} />
          ) : (
            <div className="drawer-body">
              <div className="skeleton" style={{ width: '70%', height: 18 }} />
              <div className="skeleton" style={{ width: '40%', height: 11, marginTop: 12 }} />
            </div>
          )
        ) : (
          <div className="drawer-body" key={intent.id}>
            <div className="dr-title-row">
              <span className="lane-swatch" style={{ background: laneFor(intent), height: 20 }} />
              <h2 className="dr-title display">{intent.label}</h2>
            </div>
            <div className="dr-meta">
              {projectOf(intent) && <Project name={projectOf(intent)!} />}
              <span className="mono t3">
                {relativeDay(intent.date)} · {range(intent.start_ts, intent.end_ts)}
              </span>
            </div>

            <Stats intent={intent} />
            <p className="dr-summary">{intent.summary}</p>

            <section className="dr-section">
              <h3 className="dr-h">Where you left off</h3>
              <LeftOffLine intent={intent} />
            </section>

            {selection && (
              <section className="dr-section dr-restore">
                <h3 className="dr-h">Reopen</h3>
                <RestoreChecklist payload={intent.resume_payload} insights={intent.insights} selection={selection} onChange={setSelection} />
                {selectionSize(selectAll(intent.resume_payload)) > 0 && (
                  <div className="dr-restore-foot">
                    <span className="t3">{selectionSize(selection) ? `Opens ${describeSelection(selection)}` : 'Nothing selected'}</span>
                    <button
                      className="btn btn-primary btn-sm"
                      disabled={!selectionSize(selection)}
                      onClick={() =>
                        void restoreSelection(intent.label, intent.resume_payload, selection).then(
                          // Keep only what did not open, so a retry does not open it twice.
                          ({ ok, left }) => !ok && selectionSize(left) > 0 && setSelection(left),
                        )
                      }
                    >
                      <Icon name="resume" size={14} /> Resume selected
                    </button>
                  </div>
                )}
              </section>
            )}

            <Insights intent={intent} />

            {intent.todos.length > 0 && (
              <section className="dr-section">
                <h3 className="dr-h">Open markers</h3>
                <div className="dr-todos">
                  {intent.todos.map((t, i) => (
                    <div key={i} className="dr-todo">
                      <span className={`marker marker-${t.marker.toLowerCase()} mono`}>{t.marker}</span>
                      <span className="mono ellipsis" title={t.path}>
                        {basename(t.path)}
                      </span>
                      <span className="mono t3 ellipsis">{tildify(t.path).replace(/\/[^/]+$/, '')}</span>
                    </div>
                  ))}
                </div>
              </section>
            )}

            {intent.children.length > 0 && (
              <section className="dr-section">
                <h3 className="dr-h">Within this thread</h3>
                <div className="list boxed">
                  {intent.children.map((c) => (
                    <button key={c.id} className="lrow" onClick={() => setStack((s) => [...s, c.id])}>
                      <span className="lrow-label ellipsis">{c.label}</span>
                      <span className="lrow-meta mono">{range(c.start_ts, c.end_ts)}</span>
                      <Icon name="chevron-right" size={14} className="lrow-chev" />
                    </button>
                  ))}
                </div>
              </section>
            )}

            <section className="dr-section dr-danger">
              <button
                className="btn btn-quiet btn-sm dr-forget"
                onClick={() =>
                  setConfirm({
                    title: `Forget ${relativeDay(intent.date)}?`,
                    text: 'Every thread, summary and search entry for this day is deleted from this computer. This can’t be undone.',
                    confirm: 'Forget day',
                    destructive: true,
                    onConfirm: async () => {
                      try {
                        await api.forgetDate(intent.date);
                        intentsChanged();
                        toast({ tone: 'success', text: `Forgot ${relativeDay(intent.date)}` });
                        onClose();
                      } catch (r) {
                        toast({ tone: 'error', text: 'Couldn’t forget that day', detail: errorText(r) });
                      }
                    },
                  })
                }
              >
                <Icon name="trash" size={14} /> Forget this day…
              </button>
              <button className="btn btn-quiet btn-sm" onClick={() => actions.review(intent, 'tabs')} hidden={!intent.resume_payload.urls.length}>
                <Icon name="globe" size={14} /> Reopen tabs only…
              </button>
              <span className="t3 dr-esc">
                <Keys keys={['Esc']} /> close
              </span>
            </section>
          </div>
        )}
      </aside>
      {confirm && <ConfirmDialog spec={confirm} onClose={() => setConfirm(null)} />}
    </div>
  );
}

function Stats({ intent }: { intent: Intent }) {
  const sources = Object.entries(intent.stats.sources).sort((a, b) => b[1] - a[1]);
  const total = sources.reduce((s, [, n]) => s + n, 0) || 1;
  return (
    <div className="dr-stats">
      <div className="stat">
        <span className="stat-v num">{duration(intent.stats.duration_seconds)}</span>
        <span className="stat-k">active</span>
      </div>
      <div className="stat">
        <span className="stat-v num">{duration(intent.end_ts - intent.start_ts)}</span>
        <span className="stat-k">span</span>
      </div>
      <div className="stat">
        <span className="stat-v num">{count(intent.stats.event_count)}</span>
        <span className="stat-k">events</span>
      </div>
      <div className="stat stat-wide">
        <div className="srcbar" aria-hidden="true">
          {sources.map(([k, n]) => (
            <span key={k} className={`srcbar-seg src-${k}`} style={{ flexGrow: n / total }} />
          ))}
        </div>
        <span className="stat-k">
          {sources
            .slice(0, 3)
            .map(([k, n]) => `${SOURCE_NAMES[k] ?? k} ${Math.round((n / total) * 100)}%`)
            .join(' · ')}
        </span>
      </div>
    </div>
  );
}

function Insights({ intent }: { intent: Intent }) {
  const { editor, browser, shell } = intent.insights;
  if (!editor.length && !browser.length && !shell.length) return null;
  return (
    <section className="dr-section">
      <h3 className="dr-h">Most used</h3>
      <div className="dr-insights">
        {editor.slice(0, 4).map((e, i) => (
          <div key={`e${i}`} className="ins">
            <Icon name="code" size={13} />
            <span className="mono ellipsis">
              {basename(String(e.file ?? ''))}
              {typeof e.line === 'number' && <span className="t3">:{e.line}</span>}
            </span>
            {typeof e.count === 'number' && <span className="ins-n mono">{e.count}</span>}
          </div>
        ))}
        {browser.slice(0, 4).map((b, i) => (
          <div key={`b${i}`} className="ins">
            <Icon name="globe" size={13} />
            <span className="ellipsis">{String(b.title ?? b.domain ?? '')}</span>
            {typeof b.count === 'number' && <span className="ins-n mono">{b.count}</span>}
          </div>
        ))}
        {shell.slice(0, 3).map((s, i) => (
          <div key={`s${i}`} className="ins">
            <Icon name="terminal" size={13} />
            <span className="mono ellipsis">{String(s.command ?? s.command_family ?? '')}</span>
            {typeof s.exit_code === 'number' && (
              <span className={`ins-exit mono ${s.exit_code === 0 ? '' : 'is-fail'}`}>exit {s.exit_code}</span>
            )}
          </div>
        ))}
      </div>
    </section>
  );
}
