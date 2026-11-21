import { Icon } from '../components/Icon';
import { Empty, Project } from '../components/ui';
import { api } from '../lib/api';
import { ago, basename, clock, duration, hostOf, longDate, plural, range, tildify, todayIso } from '../lib/format';
import { byRecency, hasResume, laneFor, projectOf, whereLeftOff } from '../lib/intent';
import { restoreSelection, selectAll } from '../lib/restore';
import { resumeCapture, useAsync, useCapture, useLiveVersion } from '../lib/store';
import type { Digest, Intent } from '../lib/types';
import type { MainActions } from './MainWindow';

export function TodayView({ actions }: { actions: MainActions }) {
  const live = useLiveVersion();
  const date = todayIso();
  const data = useAsync(async () => {
    const [digest, intents] = await Promise.all([api.digest(date), api.intentsFor(date)]);
    return { digest, intents };
  }, [date, live]);
  const capture = useCapture();

  if (data.loading && !data.data) return <TodaySkeleton />;
  if (data.error && !data.data) {
    return (
      <div className="page">
        <Empty icon="alert" title="Couldn’t load today" text={data.error.message}>
          <button className="btn btn-sm" onClick={data.reload}>
            Try again
          </button>
        </Empty>
      </div>
    );
  }
  const intents = data.data?.intents ?? [];
  const digest = data.data?.digest;
  const roots = intents.filter((i) => i.depth === 0);

  if (roots.length === 0) {
    const paused = capture.status?.capture_paused;
    return (
      <div className="page">
        <header className="page-head">
          <div className="page-kicker mono">{longDate(date)}</div>
          <h1 className="headline display">Nothing captured yet today</h1>
        </header>
        <Empty
          icon="today"
          title={paused ? 'Capture is paused.' : 'Work as usual; threads appear here a few minutes after you start.'}
        >
          {paused && (
            <button className="btn btn-primary btn-sm" onClick={() => void resumeCapture()}>
              Start capturing
            </button>
          )}
        </Empty>
      </div>
    );
  }

  const ordered = byRecency(roots);
  const top = digest?.top_intent_ids.length
    ? digest.top_intent_ids.map((id) => roots.find((r) => r.id === id)).filter((x): x is Intent => Boolean(x))
    : [];
  const resumeCards = [...new Set([...ordered.filter(hasResume).slice(0, 1), ...top, ...ordered.filter(hasResume)])]
    .filter(hasResume)
    // Two sessions of the same work read as duplicate cards; keep the most recent one.
    .filter(
      (i, _, all) =>
        !all.some((o) => o !== i && o.label === i.label && projectOf(o) === projectOf(i) && o.end_ts > i.end_ts),
    )
    .slice(0, 4);

  return (
    <div className="page">
      <header className="page-head">
        <div className="page-kicker mono">{longDate(date)}</div>
        {digest && <DigestBlock digest={digest} roots={roots} />}
      </header>

      <section className="page-section">
        <div className="section-head">
          <h2 className="section-title">Pick up where you left off</h2>
        </div>
        <div className="cards">
          {resumeCards.map((i) => (
            <ResumeCard key={i.id} intent={i} actions={actions} />
          ))}
        </div>
      </section>

      <section className="page-section">
        <div className="section-head">
          <h2 className="section-title">All of today</h2>
          <span className="mono t3">{plural(roots.length, 'thread')}</span>
        </div>
        <div className="list boxed">
          {ordered.map((i) => (
            <button key={i.id} className="lrow" onClick={() => actions.openIntent(i.id)}>
              <span className="lane-swatch" style={{ background: laneFor(i) }} />
              <span className="lrow-label ellipsis">{i.label}</span>
              {projectOf(i) && <span className="lrow-project">{projectOf(i)}</span>}
              <span className="lrow-meta mono">{range(i.start_ts, i.end_ts)}</span>
              <span className="lrow-dur mono">{duration(i.stats.duration_seconds)}</span>
              <Icon name="chevron-right" size={14} className="lrow-chev" />
            </button>
          ))}
        </div>
      </section>
    </div>
  );
}

/** Two-tone digest: recorded facts in primary text, inferred summary in secondary. */
function DigestBlock({ digest, roots }: { digest: Digest; roots: Intent[] }) {
  const projects = [...new Set(roots.map(projectOf).filter((p): p is string => Boolean(p)))];
  const first = Math.min(...roots.map((r) => r.start_ts));
  return (
    <div className="digest">
      <h1 className="headline display">{digest.headline}</h1>
      <p className="digest-facts num">
        {plural(digest.intent_count, 'thread')} · {duration(digest.total_duration_seconds)} active · {plural(projects.length, 'project')}
        {' · '}first activity {clock(first)}
      </p>
      <p className="digest-summary">{digest.summary}</p>
    </div>
  );
}

export function ResumeCard({ intent, actions }: { intent: Intent; actions: MainActions }) {
  const left = whereLeftOff(intent);
  const project = projectOf(intent);
  const p = intent.resume_payload;
  const artifacts: Array<{ icon: 'code' | 'globe' | 'terminal'; text: string; mono: boolean }> = [
    ...p.files.map((f) => ({ icon: 'code' as const, text: basename(f), mono: true })),
    ...p.urls.map((u) => ({ icon: 'globe' as const, text: hostOf(u), mono: false })),
    ...(p.shell.cwd ? [{ icon: 'terminal' as const, text: tildify(p.shell.cwd), mono: true }] : []),
  ];
  const shown = artifacts.slice(0, 2);
  const more = artifacts.length - shown.length;

  return (
    <article className="card" style={{ ['--lane' as string]: laneFor(intent) }}>
      <button className="card-hit" onClick={() => actions.openIntent(intent.id)} aria-label={`Details for ${intent.label}`} />
      <div className="card-top">
        <h3 className="card-title">{intent.label}</h3>
        <div className="card-meta">
          {project && <Project name={project} />}
          <span className="mono t3">
            {ago(intent.end_ts)} · {duration(intent.stats.duration_seconds)} active
          </span>
        </div>
      </div>
      {left && (
        <div className="card-left">
          <span className="card-left-label">Where you left off</span>
          <span className={`card-left-text ellipsis ${left.kind === 'tab' ? '' : 'mono'}`}>
            <Icon name={left.kind === 'file' ? 'code' : left.kind === 'tab' ? 'globe' : 'terminal'} size={13} />
            {left.kind === 'cmd' ? `$ ${left.text}` : left.text}
          </span>
        </div>
      )}
      <div className="card-chips">
        {shown.map((a, i) => (
          <span key={i} className={`chip ${a.mono ? 'chip-mono' : ''}`}>
            <Icon name={a.icon} size={12} />
            <span className="ellipsis">{a.text}</span>
          </span>
        ))}
        {more > 0 && <span className="chip chip-more num">+{more}</span>}
      </div>
      <div className="card-actions">
        <button
          className="btn btn-primary btn-sm"
          onClick={() => void restoreSelection(intent.label, p, selectAll(p))}
          title="Reopen files, tabs and terminal. No command is run."
        >
          <Icon name="resume" size={14} />
          Resume
        </button>
        <button className="btn btn-sm" onClick={() => actions.review(intent)}>
          Review
        </button>
      </div>
    </article>
  );
}

function TodaySkeleton() {
  return (
    <div className="page" aria-busy="true">
      <div className="skeleton" style={{ width: 160, height: 12 }} />
      <div className="skeleton" style={{ width: '62%', height: 22, marginTop: 14 }} />
      <div className="skeleton" style={{ width: '80%', height: 12, marginTop: 14 }} />
      <div className="skeleton" style={{ width: '74%', height: 12, marginTop: 8 }} />
      <div className="cards" style={{ marginTop: 36 }}>
        {[0, 1].map((i) => (
          <div key={i} className="card" style={{ height: 196 }} />
        ))}
      </div>
    </div>
  );
}
