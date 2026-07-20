import { useState } from 'react';
import { confidenceTone, formatDuration, projectTag } from '../lib/format';
import type { Intent } from '../types';
import { InsightChips } from './InsightChips';

interface IntentCardProps {
  intent: Intent;
  selectedId: string | null;
  onSelect: (intentId: string) => void;
  onResume: (intentId: string, preferredMode: 'resume' | 'continue') => void;
}

function includesIntent(intent: Intent, targetId: string | null): boolean {
  return targetId === intent.id || intent.children.some((child) => includesIntent(child, targetId));
}

export function IntentCard({ intent, selectedId, onSelect, onResume }: IntentCardProps) {
  const [expanded, setExpanded] = useState(false);
  const childrenVisible = expanded || includesIntent(intent, selectedId);
  const project = projectTag(intent);
  return (
    <article className={`intent-card ${selectedId === intent.id ? 'is-selected' : ''}`} role="option" aria-selected={selectedId === intent.id}>
      <button className="intent-main button-reset" type="button" onClick={() => onSelect(intent.id)}>
        <span className={`confidence-dot ${confidenceTone(intent.confidence)}`} title={`${Math.round(intent.confidence * 100)}% confidence`} />
        <span className="intent-copy">
          <span className="intent-title">{intent.label}</span>
          <span className="intent-meta">
            {project && <span>{project}</span>}
            <span>{formatDuration(intent.stats.duration_seconds)}</span>
          </span>
        </span>
      </button>
      <InsightChips intent={intent} />
      <div className="intent-actions">
        {intent.children.length > 0 && (
          <button
            className="text-button"
            type="button"
            onClick={() => setExpanded((visible) => !visible)}
            aria-expanded={childrenVisible}
          >
            {childrenVisible ? 'Hide details' : `${intent.children.length} detail${intent.children.length === 1 ? '' : 's'}`}
          </button>
        )}
        <button className="action-button primary" type="button" onClick={() => onResume(intent.id, 'resume')}>Resume</button>
        <button className="action-button" type="button" onClick={() => onResume(intent.id, 'continue')}>Continue</button>
      </div>
      {intent.children.length > 0 && childrenVisible && (
        <div className="intent-children">
          {intent.children.map((child) => (
            <IntentCard key={child.id} intent={child} selectedId={selectedId} onSelect={onSelect} onResume={onResume} />
          ))}
        </div>
      )}
    </article>
  );
}
