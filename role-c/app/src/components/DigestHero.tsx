import { formatDate, formatDuration } from '../lib/format';
import type { CurrentIntent, Digest } from '../types';

interface DigestHeroProps {
  digest: Digest;
  current: CurrentIntent | null;
}

export function DigestHero({ digest, current }: DigestHeroProps) {
  return (
    <section className="digest-hero">
      <div className="digest-eyebrow">
        <span>Yesterday · {formatDate(digest.date)}</span>
        {current && current.confidence >= 0.5 && <span className="now-pill" title={current.summary}>Now · {current.label}</span>}
      </div>
      <h1>{digest.headline}</h1>
      <p>{digest.summary}</p>
      <span className="digest-meta">
        {digest.intent_count} {digest.intent_count === 1 ? 'session' : 'sessions'} · {formatDuration(digest.total_duration_seconds)}
      </span>
    </section>
  );
}
