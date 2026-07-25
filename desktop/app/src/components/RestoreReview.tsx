import type { RestoreReview as RestoreReviewData } from '../types';

interface RestoreReviewProps {
  review: RestoreReviewData;
  restoring: boolean;
  onClose: () => void;
  onConfirm: () => void;
}

export function RestoreReview({ review, restoring, onClose, onConfirm }: RestoreReviewProps) {
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
      <div className="restore-scroll-region" aria-label="Stored restore context" tabIndex={0}>
        <p className="restore-summary">{review.summary || review.label}</p>
        <div className="restore-items">
          {payload.files.length > 0 && <RestoreItem label="Files" values={payload.files} />}
          {payload.urls.length > 0 && <RestoreItem label="Firefox tabs" values={payload.urls} />}
          {payload.shell.cwd && <RestoreItem label="Terminal folder" values={[payload.shell.cwd]} />}
          {!payload.files.length && !payload.urls.length && !payload.shell.cwd && <span>No restorable applications were stored for this session.</span>}
        </div>
        {payload.shell.last_cmd && <p className="command-note">The last command is shown for context only and will never run automatically.</p>}
      </div>
      <div className="restore-actions">
        <button className="compact-primary" type="button" onClick={onConfirm} disabled={restoring}>{restoring ? 'Opening…' : 'Resume'}</button>
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
