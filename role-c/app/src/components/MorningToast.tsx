import { formatDate } from '../lib/format';
import type { Digest } from '../types';

interface MorningToastProps {
  digest: Digest;
  visible: boolean;
  onResume: () => void;
  onDismiss: () => void;
}

export function MorningToast({ digest, visible, onResume, onDismiss }: MorningToastProps) {
  if (!visible) return null;
  return (
    <aside className="morning-toast glass" aria-label="Morning briefing">
      <div className="toast-copy">
        <strong>Yesterday · {digest.headline}</strong>
        <p title={digest.summary}>{digest.summary}</p>
        <span>{formatDate(digest.date)}</span>
      </div>
      <div className="toast-actions">
        <button type="button" className="compact-primary" onClick={onResume}>Resume</button>
        <button type="button" className="compact-quiet" onClick={onDismiss}>Dismiss</button>
      </div>
    </aside>
  );
}
