import { useEffect, useState } from 'react';
import type { RestoreReview as RestoreReviewData } from '../types';

interface RestoreReviewProps {
  review: RestoreReviewData;
  restoring: boolean;
  onClose: () => void;
  onConfirm: (mode: 'resume' | 'continue') => void;
}

export function RestoreReview({ review, restoring, onClose, onConfirm }: RestoreReviewProps) {
  const [mode, setMode] = useState<'resume' | 'continue'>(review.preferredMode ?? 'resume');
  useEffect(() => setMode(review.preferredMode ?? 'resume'), [review]);
  const { payload } = review;
  return (
    <section className="restore-review glass" role="dialog" aria-modal="false" aria-labelledby="restore-title">
      <div className="restore-heading">
        <div>
          <span className="section-kicker">Local restore</span>
          <h2 id="restore-title">Review before reopening</h2>
        </div>
        <button type="button" className="close-button" onClick={onClose} aria-label="Close restore review">×</button>
      </div>
      <p className="restore-summary">{review.summary || review.label}</p>
      <div className="restore-items" aria-label="Stored restore context">
        {payload.files.length > 0 && <RestoreItem label="Files" values={payload.files} />}
        {payload.urls.length > 0 && <RestoreItem label="Pages" values={payload.urls} />}
        {payload.shell.cwd && <RestoreItem label="Terminal folder" values={[payload.shell.cwd]} />}
        {!payload.files.length && !payload.urls.length && !payload.shell.cwd && <span>No restorable applications were stored for this session.</span>}
      </div>
      {payload.shell.last_cmd && <p className="command-note">The last command is shown for context only and will never run automatically.</p>}
      <fieldset className="restore-modes">
        <legend>Choose how to reopen it</legend>
        <label className={mode === 'resume' ? 'is-active' : ''}>
          <input type="radio" name="restore-mode" checked={mode === 'resume'} onChange={() => setMode('resume')} />
          <strong>Resume</strong><span>Open saved files, pages, and terminal folder.</span>
        </label>
        <label className={mode === 'continue' ? 'is-active' : ''}>
          <input type="radio" name="restore-mode" checked={mode === 'continue'} onChange={() => setMode('continue')} />
          <strong>Continue</strong><span>Reuse open files where possible.</span>
        </label>
      </fieldset>
      <div className="restore-actions">
        <button className="compact-quiet" type="button" onClick={onClose} disabled={restoring}>Cancel</button>
        <button className="compact-primary" type="button" onClick={() => onConfirm(mode)} disabled={restoring}>
          {restoring ? 'Opening…' : mode === 'resume' ? 'Resume' : 'Continue'}
        </button>
      </div>
    </section>
  );
}

function RestoreItem({ label, values }: { label: string; values: string[] }) {
  return (
    <div className="restore-item">
      <strong>{label}</strong>
      <ul>{values.map((value) => <li key={value} title={value}>{value}</li>)}</ul>
    </div>
  );
}
