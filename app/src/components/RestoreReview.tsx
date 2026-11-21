import { useEffect, useRef, useState } from 'react';
import { ago, duration } from '../lib/format';
import { laneFor, projectOf } from '../lib/intent';
import { describeSelection, restoreSelection, selectAll, selectionSize, type RestoreSelection } from '../lib/restore';
import type { InsightRow, ResumePayload } from '../lib/types';
import { RestoreChecklist } from './RestoreChecklist';
import { Keys, Project } from './ui';

export interface ReviewTarget {
  id: string;
  label: string;
  summary: string;
  tags: string[];
  end_ts?: number;
  duration_seconds?: number;
  payload: ResumePayload;
  insights?: { editor: InsightRow[]; browser: InsightRow[] };
  /** Pre-filter, e.g. "tabs only" from the action panel. */
  only?: 'tabs';
}

export function initialSelection(target: ReviewTarget): RestoreSelection {
  const all = selectAll(target.payload);
  return target.only === 'tabs' ? { files: [], urls: all.urls, terminal: false } : all;
}

/**
 * Restore review: every item is explicit and individually toggleable; nothing
 * opens until the user confirms. Used inline in the overlay and as a sheet in
 * the main window.
 */
export function RestoreReview({
  target,
  onCancel,
  onDone,
  variant,
}: {
  target: ReviewTarget;
  onCancel: () => void;
  onDone: () => void;
  variant: 'inline' | 'sheet';
}) {
  const [selection, setSelection] = useState<RestoreSelection>(() => initialSelection(target));
  const [busy, setBusy] = useState(false);
  const confirmRef = useRef<HTMLButtonElement>(null);
  const size = selectionSize(selection);
  const project = projectOf(target);

  // State updates land after the next render; a ref stops a second Enter or click at once.
  const running = useRef(false);

  const confirm = async () => {
    if (running.current || size === 0) return;
    running.current = true;
    setBusy(true);
    const { ok, left } = await restoreSelection(target.label, target.payload, selection);
    running.current = false;
    setBusy(false);
    // After a partial failure, keep only what did not open, so a retry does not open it twice.
    if (ok || selectionSize(left) === 0) onDone();
    else setSelection(left);
  };

  useEffect(() => {
    if (variant === 'sheet') confirmRef.current?.focus();
  }, [variant]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        onCancel();
      } else if (e.key === 'Enter' && !(e.target instanceof HTMLButtonElement)) {
        e.preventDefault();
        void confirm();
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  });

  return (
    <div className={`review review-${variant}`} role="dialog" aria-label={`Review restore: ${target.label}`}>
      <header className="review-head">
        <span className="lane-swatch review-lane" style={{ background: laneFor(target) }} />
        <div className="review-titles">
          <div className="review-kicker t2">Reopen where you left off</div>
          <h2 className="review-title display ellipsis">{target.label}</h2>
          <div className="review-meta">
            {project && <Project name={project} />}
            {target.end_ts !== undefined && <span className="mono t3">{ago(target.end_ts)}</span>}
            {target.duration_seconds !== undefined && (
              <span className="mono t3">{duration(target.duration_seconds)} active</span>
            )}
          </div>
        </div>
      </header>
      <div className="review-body">
        <RestoreChecklist payload={target.payload} insights={target.insights} selection={selection} onChange={setSelection} />
      </div>
      <footer className="review-foot">
        <span className="review-summary t2">
          {size === 0 ? 'Nothing selected' : <>Opens {describeSelection(selection)}. Nothing is closed.</>}
        </span>
        <button className="btn btn-quiet" onClick={onCancel}>
          Cancel <Keys keys={['Esc']} />
        </button>
        <button ref={confirmRef} className="btn btn-primary" disabled={busy || size === 0} onClick={() => void confirm()}>
          {busy ? 'Reopening…' : `Resume ${size} ${size === 1 ? 'item' : 'items'}`} <Keys keys={['↵']} />
        </button>
      </footer>
    </div>
  );
}
