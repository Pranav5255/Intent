import { useEffect, useLayoutEffect, useMemo, useRef, useState, type RefObject } from 'react';
import { Icon } from '../components/Icon';
import { Empty, Project } from '../components/ui';
import { api } from '../lib/api';
import { nowSec } from '../lib/env';
import { clock, duration, longDate, relativeDay, shiftIso, shortDate, todayIso } from '../lib/format';
import { laneColor, laneFor, projectOf } from '../lib/intent';
import { useAsync, useIntentsVersion, useLiveVersion } from '../lib/store';
import type { Intent } from '../lib/types';
import type { MainActions } from './MainWindow';

type Scale = 'day' | 'fortnight';

const GAP_W = 76;
const MIN_GAP = 40 * 60;
const PAD = 5 * 60;

interface Seg {
  kind: 'active' | 'gap';
  start: number;
  end: number;
  x: number;
  w: number;
}

/** Piecewise axis: active spans scale with time, long idle gaps collapse to a fixed width. */
function buildAxis(intents: Intent[], width: number, until: number | null): Seg[] {
  if (!intents.length || width <= 0) return [];
  const spans = intents
    .map((i) => [i.start_ts, Math.max(i.end_ts, i.start_ts + 60)] as [number, number])
    .sort((a, b) => a[0] - b[0]);
  if (until) spans.push([until - 60, until]);
  const merged: Array<[number, number]> = [];
  for (const [s, e] of spans) {
    const last = merged[merged.length - 1];
    if (last && s - last[1] < MIN_GAP) last[1] = Math.max(last[1], e);
    else merged.push([s, e]);
  }
  const padded = merged.map(([s, e]) => [s - PAD, e + PAD] as [number, number]);
  const gaps = padded.length - 1;
  const activeSec = padded.reduce((sum, [s, e]) => sum + (e - s), 0);
  const pps = (width - gaps * GAP_W) / activeSec;
  const segs: Seg[] = [];
  let x = 0;
  padded.forEach(([s, e], i) => {
    const w = (e - s) * pps;
    segs.push({ kind: 'active', start: s, end: e, x, w });
    x += w;
    const next = padded[i + 1];
    if (next) {
      segs.push({ kind: 'gap', start: e, end: next[0], x, w: GAP_W });
      x += GAP_W;
    }
  });
  return segs;
}

function xOf(segs: Seg[], ts: number): number {
  for (const s of segs) {
    if (ts >= s.start && ts <= s.end) return s.x + ((ts - s.start) / (s.end - s.start)) * s.w;
  }
  const last = segs[segs.length - 1];
  if (!last) return 0;
  return ts < (segs[0]?.start ?? 0) ? 0 : last.x + last.w;
}

function useWidth<T extends HTMLElement>(): [RefObject<T | null>, number] {
  const ref = useRef<T>(null);
  const [w, setW] = useState(0);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setW(el.clientWidth));
    ro.observe(el);
    setW(el.clientWidth);
    return () => ro.disconnect();
  }, []);
  return [ref, w];
}

export function TimelineView({ actions }: { actions: MainActions }) {
  const [scale, setScale] = useState<Scale>('day');
  const [date, setDate] = useState(todayIso());
  const today = todayIso();

  return (
    <div className="page page-wide">
      <header className="tl-head">
        <div>
          <div className="page-kicker mono">{scale === 'day' ? longDate(date) : `${shortDate(shiftIso(today, -13))} – ${shortDate(today)}`}</div>
          <h1 className="page-title display">Timeline</h1>
        </div>
        <div className="tl-controls">
          {scale === 'day' && (
            <div className="datenav">
              <button className="btn btn-icon btn-sm btn-quiet" aria-label="Previous day" onClick={() => setDate((d) => shiftIso(d, -1))}>
                <Icon name="chevron-left" size={14} />
              </button>
              <label className="datepick">
                <Icon name="calendar" size={14} />
                <span className="num">{relativeDay(date)}</span>
                <input
                  type="date"
                  value={date}
                  max={today}
                  onChange={(e) => e.target.value && setDate(e.target.value)}
                  aria-label="Pick a day"
                />
              </label>
              <button
                className="btn btn-icon btn-sm btn-quiet"
                aria-label="Next day"
                disabled={date >= today}
                onClick={() => setDate((d) => shiftIso(d, 1))}
              >
                <Icon name="chevron-right" size={14} />
              </button>
            </div>
          )}
          <div className="segmented" role="group" aria-label="Scale">
            <button aria-pressed={scale === 'day'} onClick={() => setScale('day')}>
              Day
            </button>
            <button aria-pressed={scale === 'fortnight'} onClick={() => setScale('fortnight')}>
              14 days
            </button>
          </div>
        </div>
      </header>
      {scale === 'day' ? (
        <DayLanes date={date} actions={actions} />
      ) : (
        <Fortnight
          onPick={(d) => {
            setDate(d);
            setScale('day');
          }}
        />
      )}
    </div>
  );
}

function DayLanes({ date, actions }: { date: string; actions: MainActions }) {
  const live = useLiveVersion();
  const data = useAsync(() => api.intentsFor(date), [date, live]);
  const [trackRef, width] = useWidth<HTMLDivElement>();
  const [hover, setHover] = useState<Intent | null>(null);
  const roots = useMemo(() => (data.data ?? []).filter((i) => i.depth === 0).sort((a, b) => a.start_ts - b.start_ts), [data.data]);
  const isToday = date === todayIso();
  const segs = useMemo(() => buildAxis(roots, width, isToday ? nowSec() : null), [roots, width, isToday]);

  useEffect(() => setHover(null), [date]);

  const ticks = useMemo(() => {
    const out: Array<{ x: number; label: string }> = [];
    for (const s of segs) {
      if (s.kind !== 'active') continue;
      // Align to local hours (timezones with :30 offsets break epoch maths).
      const d = new Date(s.start * 1000);
      d.setMinutes(0, 0, 0);
      let first = Math.floor(d.getTime() / 1000);
      if (first < s.start) first += 3600;
      for (let t = first; t <= s.end; t += 3600) {
        const x = xOf(segs, t);
        if (out.length && x - out[out.length - 1]!.x < 44) continue;
        out.push({ x, label: clock(t) });
      }
    }
    return out;
  }, [segs]);

  const total = roots.reduce((sum, r) => sum + r.stats.duration_seconds, 0);

  return (
    <div className="tl">
      <div className="tl-grid">
        <div className="tl-labels">
          <div className="tl-axis-spacer mono t3">{roots.length ? `${roots.length} threads · ${duration(total)}` : ''}</div>
          {roots.map((r) => (
            <button key={r.id} className="tl-label" onClick={() => actions.openIntent(r.id)}>
              <span className="lane-swatch" style={{ background: laneFor(r) }} />
              <span className="ellipsis">{r.label}</span>
            </button>
          ))}
        </div>
        <div className="tl-track" ref={trackRef} onMouseLeave={() => setHover(null)}>
          <div className="tl-axis">
            {ticks.map((t) => (
              <span key={t.label} className="tl-tick mono" style={{ left: t.x }}>
                {t.label}
              </span>
            ))}
          </div>
          <div className="tl-body">
            {segs
              .filter((s) => s.kind === 'gap')
              .map((s) => (
                <div key={s.start} className="tl-gap" style={{ left: s.x, width: s.w }}>
                  <span className="tl-gap-label mono" title="Idle">{duration(s.end - s.start + 2 * PAD)} idle</span>
                </div>
              ))}
            {ticks.map((t) => (
              <span key={t.label} className="tl-gridline" style={{ left: t.x }} />
            ))}
            {roots.map((r, row) => {
              const x = xOf(segs, r.start_ts);
              const w = Math.max(6, xOf(segs, r.end_ts) - x);
              return (
                <div key={r.id} className="tl-lane" style={{ top: row * 36 }}>
                  <button
                    className="tl-bar"
                    style={{ left: x, width: w, ['--lane' as string]: laneFor(r) }}
                    onMouseEnter={() => setHover(r)}
                    onFocus={() => setHover(r)}
                    onClick={() => actions.openIntent(r.id)}
                    aria-label={`${r.label}, ${clock(r.start_ts)} to ${clock(r.end_ts)}`}
                  >
                    {r.children.map((c) => (
                      <span
                        key={c.id}
                        className="tl-child"
                        style={{ left: xOf(segs, c.start_ts) - x, width: Math.max(3, xOf(segs, c.end_ts) - xOf(segs, c.start_ts)) }}
                      />
                    ))}
                  </button>
                </div>
              );
            })}
            {isToday && segs.length > 0 && (
              <div className="tl-now" style={{ left: xOf(segs, nowSec()) }}>
                <span className="tl-now-label mono">{clock(nowSec())}</span>
              </div>
            )}
            <div style={{ height: roots.length * 36 }} />
          </div>
        </div>
      </div>
      {roots.length > 0 && (
        <div className="tl-detail" aria-live="polite">
          {hover ? (
            <>
              <div className="tl-pop-title">{hover.label}</div>
              <div className="tl-pop-meta">
                {projectOf(hover) && <Project name={projectOf(hover)!} />}
                <span className="mono t3">
                  {clock(hover.start_ts)}–{clock(hover.end_ts)} · {duration(hover.stats.duration_seconds)}
                </span>
              </div>
              <p className="tl-pop-sum">{hover.summary}</p>
            </>
          ) : (
            <p className="tl-detail-hint t3">Hover a thread to see what happened · click to open it</p>
          )}
        </div>
      )}
      {!data.loading && roots.length === 0 && (
        <Empty icon="timeline" title={`Nothing recorded on ${relativeDay(date)}`} text="Pick another day, or check that capture was running." />
      )}
    </div>
  );
}

function Fortnight({ onPick }: { onPick: (date: string) => void }) {
  const today = todayIso();
  const days = useMemo(() => Array.from({ length: 14 }, (_, i) => shiftIso(today, i - 13)), [today]);
  // Fourteen requests: reload on a real change only, not on the minute timer.
  const changed = useIntentsVersion();
  const data = useAsync(async () => {
    const lists = await Promise.all(days.map((d) => api.intentsFor(d).catch(() => [] as Intent[])));
    return lists;
  }, [days.join(), changed]);
  const [hover, setHover] = useState<string | null>(null);

  const lanes = useMemo(() => {
    const map = new Map<string, number[]>();
    (data.data ?? []).forEach((list, dayIndex) => {
      for (const i of list.filter((x) => x.depth === 0)) {
        const key = projectOf(i) ?? 'unsorted';
        const row = map.get(key) ?? Array<number>(14).fill(0);
        row[dayIndex] = (row[dayIndex] ?? 0) + i.stats.duration_seconds;
        map.set(key, row);
      }
    });
    return [...map.entries()]
      .map(([project, cells]) => ({ project, cells, total: cells.reduce((a, b) => a + b, 0) }))
      .sort((a, b) => b.total - a.total);
  }, [data.data]);

  const max = Math.max(1, ...lanes.flatMap((l) => l.cells));

  return (
    <div className="ft">
      <div className="ft-row ft-axis">
        <span />
        {days.map((d) => {
          const dt = new Date(`${d}T12:00:00`);
          return (
            <span key={d} className={`ft-day mono ${d === today ? 'is-today' : ''}`}>
              <span>{['S', 'M', 'T', 'W', 'T', 'F', 'S'][dt.getDay()]}</span>
              <span className="num">{dt.getDate()}</span>
            </span>
          );
        })}
        <span className="ft-total mono t3">total</span>
      </div>
      {lanes.map((lane) => (
        <div key={lane.project} className="ft-row">
          <span className="ft-label">
            <Project name={lane.project} />
          </span>
          {lane.cells.map((sec, i) => {
            const key = `${lane.project}:${i}`;
            const d = days[i]!;
            return (
              <button
                key={key}
                className="ft-cell"
                disabled={sec === 0}
                style={{
                  ['--lane' as string]: laneColor(lane.project),
                  ['--fill' as string]: sec ? `${Math.round(18 + (sec / max) * 70)}%` : '0%',
                }}
                onMouseEnter={() => setHover(key)}
                onMouseLeave={() => setHover(null)}
                onClick={() => onPick(d)}
                aria-label={`${lane.project}, ${relativeDay(d)}: ${duration(sec)}`}
              >
                {hover === key && sec > 0 && (
                  <span className="ft-tip mono">
                    {relativeDay(d)} · {duration(sec)}
                  </span>
                )}
              </button>
            );
          })}
          <span className="ft-total mono">{duration(lane.total)}</span>
        </div>
      ))}
      <p className="ft-note t3">Older days are shown by day. Select a day to see it hour by hour.</p>
    </div>
  );
}
