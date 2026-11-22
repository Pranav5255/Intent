import type { SourceStatus } from './types';

/** The capture sources each companion sends. Firefox and Chrome are one browser row. */
export const BROWSER_SOURCES = ['firefox', 'chrome'];

/** One status for several sources: counts add up, the newest event wins, healthy if any is. */
export function combinedStatus(sources: Record<string, SourceStatus> | undefined, keys: string[]): SourceStatus | undefined {
  const found = keys.map((k) => sources?.[k]).filter((s): s is SourceStatus => Boolean(s));
  if (!found.length) return undefined;
  const newest = (pick: (s: SourceStatus) => number | null) =>
    found.reduce<number | null>((best, s) => {
      const v = pick(s);
      return v !== null && (best === null || v > best) ? v : best;
    }, null);
  const latest = found.reduce((a, b) => ((b.last_event_ts ?? -1) > (a.last_event_ts ?? -1) ? b : a));
  return {
    event_count: found.reduce((sum, s) => sum + s.event_count, 0),
    last_event_ts: newest((s) => s.last_event_ts),
    last_ingested_at: newest((s) => s.last_ingested_at),
    last_ingest_lag_seconds: latest.last_ingest_lag_seconds,
    healthy: found.some((s) => s.healthy),
  };
}
