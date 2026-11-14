import { api } from './api';
import { plural } from './format';
import { errorText, toast, updateToast } from './store';
import type { ResumePayload } from './types';

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
 * Restore only the selected items. The last shell command is never sent for
 * execution: only the working directory is passed so a terminal opens there.
 */
export async function restoreSelection(label: string, payload: ResumePayload, sel: RestoreSelection): Promise<boolean> {
  const size = selectionSize(sel);
  if (size === 0) {
    toast({ tone: 'neutral', text: 'Nothing selected to reopen.' });
    return false;
  }
  const id = toast({ tone: 'progress', text: `Reopening ${describeSelection(sel)}…`, detail: label });
  try {
    const result = await api.restore({
      mode: 'resume',
      files: sel.files,
      urls: sel.urls,
      shell: sel.terminal ? { cwd: payload.shell.cwd ?? null } : {},
    });
    const opened: RestoreSelection = {
      files: sel.files.slice(0, result.restored.files),
      urls: sel.urls.slice(0, result.restored.urls),
      terminal: result.restored.shell,
    };
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
    return result.ok;
  } catch (reason) {
    updateToast(id, { tone: 'error', text: 'Couldn’t reopen this work', detail: errorText(reason) }, 8000);
    return false;
  }
}
