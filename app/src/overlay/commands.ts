import type { IconName } from '../components/Icon';
import { clock } from '../lib/format';
import { pauseUntil } from '../lib/store';

export type CommandId =
  | 'ask'
  | 'pause-15m'
  | 'pause-1h'
  | 'pause-tomorrow'
  | 'resume-capture'
  | 'run-pipeline'
  | 'open-today'
  | 'open-timeline'
  | 'open-search'
  | 'open-settings';

export interface Command {
  id: CommandId;
  title: string;
  subtitle?: string;
  icon: IconName;
  keywords: string;
  hint?: string;
}

export function commandList(paused: boolean): Command[] {
  const list: Command[] = [];
  if (paused) {
    list.push({ id: 'resume-capture', title: 'Resume capture', icon: 'play', keywords: 'resume start unpause capture record' });
  }
  list.push(
    { id: 'pause-15m', title: 'Pause capture for 15 minutes', icon: 'pause', keywords: 'pause stop capture privacy 15m', hint: `until ${clock(pauseUntil('15m'))}` },
    { id: 'pause-1h', title: 'Pause capture for 1 hour', icon: 'pause', keywords: 'pause stop capture privacy hour 1h', hint: `until ${clock(pauseUntil('1h'))}` },
    { id: 'pause-tomorrow', title: 'Pause capture until tomorrow', icon: 'pause', keywords: 'pause stop capture privacy tomorrow day', hint: 'until 08:00' },
    { id: 'run-pipeline', title: 'Run pipeline now', subtitle: 'Group today’s activity into threads', icon: 'refresh', keywords: 'run pipeline process refresh group now' },
    { id: 'open-today', title: 'Open dashboard', icon: 'today', keywords: 'open dashboard today digest main window' },
    { id: 'open-timeline', title: 'Open timeline', icon: 'timeline', keywords: 'open timeline day lanes history' },
    { id: 'open-search', title: 'Search history', icon: 'search', keywords: 'search history find' },
    { id: 'open-settings', title: 'Settings', icon: 'settings', keywords: 'settings preferences options privacy capture provider' },
    { id: 'ask', title: 'Ask about your work', icon: 'ask', keywords: 'ask question copilot answer', hint: '?' },
  );
  return list;
}

/** Commands shown with an empty query, in order. */
export function defaultCommands(paused: boolean): Command[] {
  const all = commandList(paused);
  const pick: CommandId[] = paused ? ['resume-capture', 'open-today', 'open-settings'] : ['pause-1h', 'open-today', 'open-settings'];
  return pick.map((id) => all.find((c) => c.id === id)!).filter(Boolean);
}

export type ItemActionId =
  | 'resume'
  | 'review'
  | 'tabs-only'
  | 'open'
  | 'copy-summary'
  | 'copy-context'
  | 'forget-day'
  | 'forget-project';

export interface ItemAction {
  id: ItemActionId;
  title: string;
  icon: IconName;
  keys?: string[];
  destructive?: boolean;
}

export function itemActions(opts: { project: string | null; hasTabs: boolean; date: string }): ItemAction[] {
  const list: ItemAction[] = [
    { id: 'resume', title: 'Resume', icon: 'resume', keys: ['↵'] },
    { id: 'review', title: 'Review restore…', icon: 'review', keys: ['mod', '↵'] },
  ];
  if (opts.hasTabs) list.push({ id: 'tabs-only', title: 'Reopen tabs only…', icon: 'globe' });
  list.push(
    { id: 'open', title: 'Show details in Intent', icon: 'external', keys: ['mod', 'O'] },
    { id: 'copy-summary', title: 'Copy summary', icon: 'copy', keys: ['mod', 'C'] },
    { id: 'copy-context', title: 'Copy context as Markdown', icon: 'markdown', keys: ['mod', '⇧', 'C'] },
    { id: 'forget-day', title: 'Forget this day…', icon: 'trash', destructive: true },
  );
  if (opts.project) list.push({ id: 'forget-project', title: `Forget project “${opts.project}”…`, icon: 'trash', destructive: true });
  return list;
}
