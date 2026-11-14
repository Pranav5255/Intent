import { basename, hostOf, tildify } from './format';
import type { Intent, ResumePayload } from './types';

export const LANES = ['#7FA7E0', '#6FBF9A', '#D7A35F', '#D98580', '#A992D9', '#5DBDC4', '#C7B25B', '#C98BB8'];

/** Stable lane colour per project (or label when untagged). */
export function laneColor(key: string): string {
  let hash = 0;
  for (let i = 0; i < key.length; i += 1) hash = (hash * 31 + key.charCodeAt(i)) >>> 0;
  return LANES[hash % LANES.length] ?? LANES[0]!;
}

export function projectOf(intent: Pick<Intent, 'tags'>): string | null {
  return intent.tags.find((t) => t.startsWith('project:'))?.slice('project:'.length) ?? null;
}

export function laneFor(intent: Pick<Intent, 'tags' | 'label'>): string {
  return laneColor(projectOf(intent) ?? intent.label);
}

export function flatten(intents: Intent[]): Intent[] {
  return intents.flatMap((i) => [i, ...flatten(i.children)]);
}

export function payloadSize(p: ResumePayload): number {
  return p.files.length + p.urls.length + (p.shell.cwd ? 1 : 0);
}

export function hasResume(intent: Intent): boolean {
  return payloadSize(intent.resume_payload) > 0;
}

export type LeftOff =
  | { kind: 'file'; text: string; detail?: string }
  | { kind: 'tab'; text: string; detail?: string }
  | { kind: 'cmd'; text: string; detail?: string };

/**
 * One "where you left off" line, chosen from the source the intent leaned on
 * most: last file:line for editor work, last tab title for browsing, else the
 * last shell command (shown, never run).
 */
export function whereLeftOff(intent: Intent): LeftOff | null {
  const src = intent.stats.sources;
  const editorWeight = src.vscode ?? 0;
  const browserWeight = (src.firefox ?? 0) + (src.chrome ?? 0);
  const firstFile = intent.insights.editor[0];
  const file = intent.resume_payload.files[0];
  const tab = intent.insights.browser[0];
  const cmd = intent.resume_payload.shell.last_cmd;

  const fileLine = (): LeftOff | null => {
    const path = (firstFile?.file as string | undefined) ?? file;
    if (!path) return null;
    const line = typeof firstFile?.line === 'number' ? `:${firstFile.line}` : '';
    return { kind: 'file', text: `${basename(path)}${line}`, detail: tildify(path) };
  };
  const tabLine = (): LeftOff | null => {
    const url = (tab?.url as string | undefined) ?? intent.resume_payload.urls[0];
    if (!url && !tab) return null;
    const title = (tab?.title as string | undefined) ?? hostOf(url ?? '');
    return { kind: 'tab', text: title, detail: url ? hostOf(url) : undefined };
  };
  const cmdLine = (): LeftOff | null => (cmd ? { kind: 'cmd', text: cmd } : null);

  const order = editorWeight >= browserWeight ? [fileLine, cmdLine, tabLine] : [tabLine, fileLine, cmdLine];
  for (const pick of order) {
    const found = pick();
    if (found) return found;
  }
  return null;
}

/** Root intents most recent first. */
export function byRecency(intents: Intent[]): Intent[] {
  return [...intents].sort((a, b) => b.end_ts - a.end_ts);
}

export function insightTitle(url: string, intent: Intent): string {
  const row = intent.insights.browser.find((b) => b.url === url);
  return (row?.title as string | undefined) ?? hostOf(url);
}

export function fileLine(path: string, intent: Intent): string | null {
  const row = intent.insights.editor.find((e) => e.file === path);
  return typeof row?.line === 'number' ? String(row.line) : null;
}
