import { api } from './api';
import { plural } from './format';
import { errorText, toast, updateToast } from './store';
import type { RestoreResult, ResumePayload } from './types';

export interface RestoreSelection {
  files: string[];
  urls: string[];
  terminal: boolean;
}

export function selectAll(payload: ResumePayload): RestoreSelection {
  return { files: [...payload.files], urls: [...payload.urls], terminal: Boolean(payload.shell.cwd) };
}

export function selectionSize(sel: RestoreSelection): number {
  return sel.files.length + sel.urls.length + (sel.terminal ? 1 : 0);
}

export function describeSelection(sel: RestoreSelection): string {
  const parts: string[] = [];
  if (sel.files.length) parts.push(plural(sel.files.length, 'file'));
  if (sel.urls.length) parts.push(plural(sel.urls.length, 'tab'));
  if (sel.terminal) parts.push('a terminal');
  if (parts.length <= 1) return parts[0] ?? 'nothing';
  return `${parts.slice(0, -1).join(', ')} and ${parts[parts.length - 1]}`;
}

/**
 * Split a selection by what the daemon reports as opened. The daemon counts
 * per kind: one editor launch opens every file that exists, and a missing file
 * fails again on a retry. So a kind with any count is treated as done.
 */
export function splitResult(sel: RestoreSelection, result: RestoreResult): { opened: RestoreSelection; left: RestoreSelection } {
  const files = result.restored.files > 0;
  const urls = result.restored.urls > 0;
  const terminal = sel.terminal && result.restored.shell;
  return {
    // For the message only: the counts, not which items.
    opened: {
      files: sel.files.slice(0, result.restored.files),
      urls: sel.urls.slice(0, result.restored.urls),
      terminal,
    },
    left: { files: files ? [] : sel.files, urls: urls ? [] : sel.urls, terminal: sel.terminal && !terminal },
  };
}

export interface RestoreOutcome {
  ok: boolean;
  /** What is still to open after a partial failure. */
  left: RestoreSelection;
}

// A double-click or a second Enter must not open everything twice.
const inFlight = new Set<string>();

/**
 * Restore only the selected items. The last shell command is never sent for
 * execution: only the working directory is passed so a terminal opens there.
 * A second call for the same items while the first runs does nothing.
 */
export async function restoreSelection(label: string, payload: ResumePayload, sel: RestoreSelection): Promise<RestoreOutcome> {
  const size = selectionSize(sel);
  if (size === 0) {
    toast({ tone: 'neutral', text: 'Nothing selected to reopen.' });
    return { ok: false, left: sel };
  }
  const key = JSON.stringify([label, sel]);
  if (inFlight.has(key)) return { ok: false, left: sel };
  inFlight.add(key);
  const id = toast({ tone: 'progress', text: `Reopening ${describeSelection(sel)}…`, detail: label });
  try {
    const result = await api.restore({
      mode: 'resume',
      files: sel.files,
      urls: sel.urls,
      shell: sel.terminal ? { cwd: payload.shell.cwd ?? null } : {},
    });
    const { opened, left } = splitResult(sel, result);
    if (result.ok && result.failed.length === 0) {
      updateToast(id, {
        tone: 'success',
        text: `Reopened ${describeSelection(opened)}`,
        detail: 'Nothing was closed and no command was run.',
      });
    } else {
      updateToast(
        id,
        {
          tone: 'error',
          text: selectionSize(opened) ? `Reopened ${describeSelection(opened)}, with problems` : 'Couldn’t reopen anything',
          detail: result.failed.join(' · '),
        },
        8000,
      );
    }
    return { ok: result.ok, left };
  } catch (reason) {
    updateToast(id, { tone: 'error', text: 'Couldn’t reopen this work', detail: errorText(reason) }, 8000);
    return { ok: false, left: sel };
  } finally {
    inFlight.delete(key);
  }
}
