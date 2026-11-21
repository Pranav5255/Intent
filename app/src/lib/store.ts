// Small shared stores (no state library): capture status and toasts.

import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { api, ApiError, TOKEN_REFUSED } from './api';
import { emit, listen } from './bridge';
import { nowMs, nowSec } from './env';
import { clock } from './format';
import type { CaptureStatus } from './types';

function createStore<T>(initial: T) {
  let value = initial;
  const subs = new Set<() => void>();
  return {
    get: () => value,
    set: (next: T | ((prev: T) => T)) => {
      value = typeof next === 'function' ? (next as (p: T) => T)(value) : next;
      subs.forEach((fn) => fn());
    },
    subscribe: (fn: () => void) => {
      subs.add(fn);
      return () => subs.delete(fn);
    },
  };
}

// ---------------------------------------------------------------------------
// Capture status
// ---------------------------------------------------------------------------

export interface CaptureState {
  status: CaptureStatus | null;
  reachable: boolean | null;
  /** The daemon answered 401 even after the app re-read its token. */
  denied: boolean;
  loading: boolean;
  /** Counts each return of the daemon after it was down or refused the token. */
  reconnects: number;
}

const captureStore = createStore<CaptureState>({
  status: null,
  reachable: null,
  denied: false,
  loading: true,
  reconnects: 0,
});
let pollTimer: number | null = null;
let pollers = 0;

export async function refreshCapture(): Promise<void> {
  try {
    const status = await api.status();
    captureStore.set((prev) => ({
      status,
      reachable: true,
      denied: false,
      loading: false,
      reconnects: prev.reachable === false || prev.denied ? prev.reconnects + 1 : prev.reconnects,
    }));
  } catch (reason) {
    const unreachable = reason instanceof ApiError ? reason.unreachable : true;
    // The Rust side already re-read the token once before this 401.
    const denied = reason instanceof ApiError && reason.status === 401;
    captureStore.set((prev) => ({ ...prev, reachable: !unreachable, denied, loading: false }));
  }
}

export function useCapture(): CaptureState {
  useEffect(() => {
    pollers += 1;
    if (pollers === 1) {
      void refreshCapture();
      pollTimer = window.setInterval(() => void refreshCapture(), 15_000);
    }
    // The tray changes pause state from Rust; it emits this so every window refreshes.
    const unlisten = listen('capture-changed', () => void refreshCapture());
    return () => {
      pollers -= 1;
      if (pollers === 0 && pollTimer !== null) window.clearInterval(pollTimer);
      void unlisten.then((fn) => fn());
    };
  }, []);
  return useSyncExternalStore(captureStore.subscribe, captureStore.get);
}

/** Reload the given data when the daemon comes back, if it failed while it was away. */
export function useRetryOnReconnect(...states: Array<Pick<AsyncState<unknown>, 'error' | 'reload'>>): void {
  const { reconnects } = useCapture();
  const latest = useRef(states);
  latest.current = states;
  useEffect(() => {
    if (reconnects === 0) return;
    for (const s of latest.current) if (s.error) s.reload();
  }, [reconnects]);
}

// ---------------------------------------------------------------------------
// Intents changed
// ---------------------------------------------------------------------------

// Forget, a pipeline run and the like change intents in every window. Each
// window bumps its own counter and tells the other windows by an app event.
const INTENTS_EVENT = 'intents-changed';
const WINDOW_ID = Math.random().toString(36).slice(2);
const intentsStore = createStore(0);
let intentsListening = false;

export function intentsChanged(): void {
  intentsStore.set((n) => n + 1);
  void emit(INTENTS_EVENT, WINDOW_ID).catch(() => undefined);
}

export function useIntentsVersion(): number {
  useEffect(() => {
    if (intentsListening) return;
    intentsListening = true;
    void listen<string>(INTENTS_EVENT, (from) => from !== WINDOW_ID && intentsStore.set((n) => n + 1));
  }, []);
  return useSyncExternalStore(intentsStore.subscribe, intentsStore.get);
}

const LIVE_INTERVAL_MS = 60_000;
const FOCUS_GAP_MS = 5_000;

/**
 * A counter for views that show live data. It changes when intents change,
 * when the daemon comes back, when the window gets focus or is shown, and
 * once a minute while the window is visible. Nothing reloads while hidden.
 */
export function useLiveVersion(): number {
  const intents = useIntentsVersion();
  const { reconnects } = useCapture();
  const [tick, setTick] = useState(0);
  useEffect(() => {
    let last = nowMs();
    const bump = (minGap: number) => {
      if (document.visibilityState === 'hidden' || nowMs() - last < minGap) return;
      last = nowMs();
      setTick((t) => t + 1);
    };
    const onTimer = () => bump(0);
    const onShow = () => bump(FOCUS_GAP_MS);
    const timer = window.setInterval(onTimer, LIVE_INTERVAL_MS);
    window.addEventListener('focus', onShow);
    document.addEventListener('visibilitychange', onShow);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener('focus', onShow);
      document.removeEventListener('visibilitychange', onShow);
    };
  }, []);
  return intents + reconnects + tick;
}

export type PauseSpan = '15m' | '1h' | 'tomorrow';

export function pauseUntil(span: PauseSpan): number {
  const now = nowSec();
  if (span === '15m') return now + 15 * 60;
  if (span === '1h') return now + 60 * 60;
  const t = new Date(nowMs());
  t.setDate(t.getDate() + 1);
  t.setHours(8, 0, 0, 0);
  return Math.floor(t.getTime() / 1000);
}

export async function pauseCapture(span: PauseSpan): Promise<number> {
  const until = pauseUntil(span);
  await api.pause(true, until);
  await refreshCapture();
  return until;
}

export async function resumeCapture(): Promise<void> {
  await api.pause(false);
  await refreshCapture();
}

export function healthySources(status: CaptureStatus | null): number {
  if (!status) return 0;
  return Object.values(status.sources).filter((s) => s.healthy).length;
}

export interface CaptureSummary {
  tone: 'capturing' | 'paused' | 'idle' | 'offline';
  text: string;
}

export function summarizeCapture(state: CaptureState): CaptureSummary {
  if (state.reachable === false) return { tone: 'offline', text: 'Intent isn’t running' };
  if (state.denied) return { tone: 'offline', text: TOKEN_REFUSED };
  const s = state.status;
  if (!s) return { tone: 'idle', text: 'Checking capture…' };
  if (s.capture_paused) {
    return {
      tone: 'paused',
      text: s.capture_paused_until ? `Paused until ${pausedUntilLabel(s.capture_paused_until)}` : 'Paused',
    };
  }
  const n = healthySources(s);
  if (n === 0) return { tone: 'idle', text: 'Waiting for activity' };
  return { tone: 'capturing', text: `Capturing · ${n} ${n === 1 ? 'source' : 'sources'}` };
}

export function pausedUntilLabel(ts: number): string {
  const until = new Date(ts * 1000);
  const today = new Date(nowMs());
  const sameDay = until.toDateString() === today.toDateString();
  return sameDay ? clock(ts) : `tomorrow ${clock(ts)}`;
}

// ---------------------------------------------------------------------------
// Toasts
// ---------------------------------------------------------------------------

export interface Toast {
  id: number;
  text: string;
  detail?: string;
  tone: 'neutral' | 'success' | 'error' | 'progress';
  action?: { label: string; run: () => void };
}

const toastStore = createStore<Toast[]>([]);
let toastSeq = 0;

export function toast(t: Omit<Toast, 'id'>, ttl = 5000): number {
  toastSeq += 1;
  const id = toastSeq;
  toastStore.set((prev) => [...prev.slice(-2), { ...t, id }]);
  if (t.tone !== 'progress') window.setTimeout(() => dismissToast(id), ttl);
  return id;
}

export function updateToast(id: number, t: Omit<Toast, 'id'>, ttl = 5000): void {
  toastStore.set((prev) => prev.map((x) => (x.id === id ? { ...t, id } : x)));
  if (t.tone !== 'progress') window.setTimeout(() => dismissToast(id), ttl);
}

export function dismissToast(id: number): void {
  toastStore.set((prev) => prev.filter((x) => x.id !== id));
}

export function useToasts(): Toast[] {
  return useSyncExternalStore(toastStore.subscribe, toastStore.get);
}

export function errorText(reason: unknown, fallback = 'Something went wrong.'): string {
  if (reason instanceof Error && reason.message) return reason.message;
  if (typeof reason === 'string') return reason;
  return fallback;
}

// ---------------------------------------------------------------------------
// Async data
// ---------------------------------------------------------------------------

export interface AsyncState<T> {
  data: T | null;
  error: ApiError | Error | null;
  loading: boolean;
  reload: () => void;
  setData: (fn: (prev: T | null) => T | null) => void;
}

export function useAsync<T>(load: () => Promise<T>, deps: unknown[]): AsyncState<T> {
  const [data, setDataState] = useState<T | null>(null);
  const [error, setError] = useState<ApiError | Error | null>(null);
  const [loading, setLoading] = useState(true);
  const [tick, setTick] = useState(0);
  const loader = useRef(load);
  loader.current = load;

  useEffect(() => {
    let live = true;
    setLoading(true);
    loader
      .current()
      .then((value) => {
        if (!live) return;
        setDataState(value);
        setError(null);
      })
      .catch((reason: unknown) => {
        if (!live) return;
        setError(reason instanceof Error ? reason : new Error(String(reason)));
      })
      .finally(() => live && setLoading(false));
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, tick]);

  const reload = useCallback(() => setTick((t) => t + 1), []);
  const setData = useCallback((fn: (prev: T | null) => T | null) => setDataState((prev) => fn(prev)), []);
  return { data, error, loading, reload, setData };
}
