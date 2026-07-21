import { cleanHighlight, formatDate } from '../lib/format';
import type { CopilotResponse, DashboardData, ResumeCandidate, SearchResult } from '../types';
import { DigestHero } from './DigestHero';
import { IntentCard } from './IntentCard';

export type DashboardContent =
  | { kind: 'timeline' }
  | { kind: 'search'; query: string; results: SearchResult[]; loading: boolean }
  | { kind: 'copilot'; question: string; response: CopilotResponse | null; loading: boolean; unavailable: boolean };

interface SessionDashboardProps {
  data: DashboardData | null;
  content: DashboardContent;
  loading: boolean;
  error: string | null;
  selectedId: string | null;
  resumeCandidates: ResumeCandidate[] | null;
  onRetry: () => void;
  onSelect: (id: string) => void;
  onReview: (id: string) => void;
  onContinue: (id: string) => void;
  onCopilotResume: (response: CopilotResponse) => void;
}

export function SessionDashboard({
  data,
  content,
  loading,
  error,
  selectedId,
  resumeCandidates,
  onRetry,
  onSelect,
  onReview,
  onContinue,
  onCopilotResume,
}: SessionDashboardProps) {
  return (
    <section id="intent-dashboard" className="session-dashboard glass" aria-label="Intent sessions">
      {loading && !data ? <LoadingState /> : error && !data ? <ServiceError error={error} onRetry={onRetry} /> : null}
      {data && <DigestHero digest={data.digest} current={data.current} />}
      {resumeCandidates && <ResumePicker candidates={resumeCandidates} onReview={onReview} />}
      {content.kind === 'search' && <SearchResults {...content} selectedId={selectedId} onSelect={onSelect} onReview={onReview} />}
      {content.kind === 'copilot' && <CopilotPanel {...content} onResume={onCopilotResume} />}
      {content.kind === 'timeline' && data && (
        <section className="sessions-section" aria-label="Yesterday's sessions">
          <div className="section-heading"><span>Sessions</span><span>{formatDate(data.digest.date)}</span></div>
          {data.intents.length ? (
            <div className="intent-list" role="listbox" aria-label="Restorable sessions">
              {data.intents.map((intent) => (
                <IntentCard key={intent.id} intent={intent} selectedId={selectedId} onSelect={onSelect} onReview={onReview} onContinue={onContinue} />
              ))}
            </div>
          ) : <EmptyState message="No sessions found. Seed Role B with POST /pipeline/run-replay." />}
        </section>
      )}
    </section>
  );
}

function SearchResults({ query, results, loading, selectedId, onSelect, onReview }: Extract<DashboardContent, { kind: 'search' }> & {
  selectedId: string | null;
  onSelect: (id: string) => void;
  onReview: (id: string) => void;
}) {
  if (!query) return null;
  return (
    <section className="sessions-section" aria-label="Search results">
      <div className="section-heading"><span>Search results</span><span>{loading ? 'Searching…' : results.length}</span></div>
      {loading ? <LoadingState compact /> : results.length ? (
        <div className="search-list" role="listbox" aria-label={`Search results for ${query}`}>
          {results.map((result) => (
            <article className={`search-result ${selectedId === result.id ? 'is-selected' : ''}`} key={result.id} role="option" aria-selected={selectedId === result.id}>
              <button className="search-main button-reset" type="button" onClick={() => onSelect(result.id)}>
                <strong>{result.label}</strong>
                <span>{cleanHighlight(result.highlight_snippet || result.summary)}</span>
                <time>{formatDate(result.date)}</time>
              </button>
              <button className="action-button" type="button" onClick={() => onReview(result.id)}>Review restore</button>
            </article>
          ))}
        </div>
      ) : <EmptyState message="No stored session matched that search." />}
    </section>
  );
}

function CopilotPanel({ question, response, loading, unavailable, onResume }: Extract<DashboardContent, { kind: 'copilot' }> & {
  onResume: (response: CopilotResponse) => void;
}) {
  return (
    <section className="copilot-panel" aria-label="Intent Copilot response">
      <div className="section-heading"><span>Copilot</span><span>{loading ? 'Thinking…' : 'Stored evidence only'}</span></div>
      {loading && <LoadingState compact />}
      {!loading && unavailable && <EmptyState message="Copilot is not configured. Your timeline and search are still available." />}
      {!loading && response && (
        <>
          <p className="copilot-question">{question}</p>
          <p className="copilot-answer">{response.answer}</p>
          {response.evidence_status === 'insufficient' && <p className="notice">There was not enough stored evidence to answer that confidently. Try a narrower question.</p>}
          {response.citations.length > 0 && (
            <div className="citations">
              {response.citations.map((citation) => <span key={citation.intent_id}>{citation.label} · {formatDate(citation.date)}</span>)}
            </div>
          )}
          {response.resume_proposal && <button className="compact-primary" type="button" onClick={() => onResume(response)}>Review stored restore context</button>}
        </>
      )}
    </section>
  );
}

function ResumePicker({ candidates, onReview }: { candidates: ResumeCandidate[]; onReview: (id: string) => void }) {
  return (
    <section className="resume-picker" aria-label="Choose a session to restore">
      <strong>Choose a stored session before restoring</strong>
      <p>Several sessions matched. Select one to review its exact saved context.</p>
      {candidates.map((candidate) => (
        <button type="button" className="picker-item" key={candidate.intent_id} onClick={() => onReview(candidate.intent_id)}>
          <span>{candidate.label}</span><small>{candidate.project_tag ?? 'Stored session'}</small>
        </button>
      ))}
    </section>
  );
}

function LoadingState({ compact = false }: { compact?: boolean }) {
  return <div className={`loading-state ${compact ? 'compact' : ''}`}><span className="loader" />Loading saved work…</div>;
}

function EmptyState({ message }: { message: string }) {
  return <div className="empty-state">{message}</div>;
}

function ServiceError({ error, onRetry }: { error: string; onRetry: () => void }) {
  return <div className="service-error"><strong>Role B unavailable</strong><span>{error}</span><button type="button" className="text-button" onClick={onRetry}>Retry</button></div>;
}
