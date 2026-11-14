import { nowMs, nowSec } from './env';

export function isoDate(date: Date): string {
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, '0');
  const d = String(date.getDate()).padStart(2, '0');
  return `${y}-${m}-${d}`;
}

export function todayIso(): string {
  return isoDate(new Date(nowMs()));
}

export function shiftIso(date: string, days: number): string {
  const parsed = new Date(`${date}T12:00:00`);
  parsed.setDate(parsed.getDate() + days);
  return isoDate(parsed);
}

export function daysAgoIso(days: number): string {
  return shiftIso(todayIso(), -days);
}

/** 47m, 1h 52m, 3h */
export function duration(seconds: number): string {
  const minutes = Math.max(0, Math.round(seconds / 60));
  if (minutes < 1) return '<1m';
  if (minutes < 60) return `${minutes}m`;
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  return m ? `${h}h ${m}m` : `${h}h`;
}

/** 2h ago, 12m ago, yesterday, 3d ago */
export function ago(ts: number): string {
  const delta = Math.max(0, nowSec() - ts);
  if (delta < 60) return 'just now';
  if (delta < 3600) return `${Math.floor(delta / 60)}m ago`;
  if (delta < 86_400) return `${Math.floor(delta / 3600)}h ago`;
  const days = Math.floor(delta / 86_400);
  if (days === 1) return 'yesterday';
  return `${days}d ago`;
}

export function clock(ts: number): string {
  const d = new Date(ts * 1000);
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

export function range(start: number, end: number): string {
  return `${clock(start)}–${clock(end)}`;
}

export function longDate(date: string): string {
  const parsed = new Date(`${date}T12:00:00`);
  if (Number.isNaN(parsed.getTime())) return date;
  return new Intl.DateTimeFormat('en-GB', { weekday: 'long', day: 'numeric', month: 'long' }).format(parsed);
}

export function shortDate(date: string): string {
  const parsed = new Date(`${date}T12:00:00`);
  if (Number.isNaN(parsed.getTime())) return date;
  return new Intl.DateTimeFormat('en-GB', { day: 'numeric', month: 'short' }).format(parsed);
}

/** "Today", "Yesterday", "Thu 12 Nov" */
export function relativeDay(date: string): string {
  if (date === todayIso()) return 'Today';
  if (date === daysAgoIso(1)) return 'Yesterday';
  const parsed = new Date(`${date}T12:00:00`);
  return new Intl.DateTimeFormat('en-GB', { weekday: 'short', day: 'numeric', month: 'short' }).format(parsed);
}

export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let value = n / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}

export function count(n: number): string {
  return new Intl.NumberFormat('en-US').format(n);
}

export function plural(n: number, word: string, many = `${word}s`): string {
  return `${n} ${n === 1 ? word : many}`;
}

export function basename(path: string): string {
  const clean = path.replace(/[/\\]+$/, '');
  const idx = Math.max(clean.lastIndexOf('/'), clean.lastIndexOf('\\'));
  return idx >= 0 ? clean.slice(idx + 1) : clean;
}

/** Collapse $HOME-like prefixes for display. */
export function tildify(path: string): string {
  return path.replace(/^\/(home|Users)\/[^/]+/, '~');
}

export function hostOf(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, '');
  } catch {
    return url;
  }
}

export function pathOf(url: string): string {
  try {
    const parsed = new URL(url);
    const tail = parsed.pathname === '/' ? '' : parsed.pathname;
    return `${parsed.hostname.replace(/^www\./, '')}${tail}`;
  } catch {
    return url;
  }
}
