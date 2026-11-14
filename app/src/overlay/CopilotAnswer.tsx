import { Icon } from '../components/Icon';
import { Keys } from '../components/ui';
import { laneColor } from '../lib/intent';
import { relativeDay } from '../lib/format';
import type { CitedIntent, CopilotQueryResponse } from '../lib/types';

export type CopilotState =
  | { status: 'idle' }
  | { status: 'loading'; question: string }
  | { status: 'done'; question: string; response: CopilotQueryResponse }
  | { status: 'unavailable'; question: string }
  | { status: 'error'; question: string; message: string };

/** Answer text with [n] markers rendered as clickable citation tokens. */
function AnswerText({ text, citations, onCite }: { text: string; citations: CitedIntent[]; onCite: (c: CitedIntent) => void }) {
  const parts = text.split(/(\[\d+\])/g);
  return (
    <p className="cp-answer">
      {parts.map((part, i) => {
        const m = part.match(/^\[(\d+)\]$/);
        const cite = m ? citations[Number(m[1]) - 1] : undefined;
        if (!m || !cite) return <span key={i}>{part}</span>;
        return (
          <button key={i} className="cite" title={cite.label} onClick={() => onCite(cite)}>
            {m[1]}
          </button>
        );
      })}
    </p>
  );
}

export function CopilotPanel({
  state,
  question,
  selectedCitation,
  onCite,
  onHoverCitation,
  onResume,
  citationLane,
}: {
  state: CopilotState;
  question: string;
  selectedCitation: number;
  onCite: (c: CitedIntent) => void;
  onHoverCitation: (i: number) => void;
  onResume: (intentId: string) => void;
  citationLane: (c: CitedIntent) => string | null;
}) {
  if (state.status === 'idle' || (state.status !== 'loading' && state.question !== question.trim())) {
    return (
      <div className="cp cp-hint">
        <div className="cp-hint-row">
          <Icon name="ask" />
          <span>{question.trim() ? <>Ask “{question.trim()}”</> : 'Ask about anything you worked on in the last two weeks'}</span>
          <span className="cp-hint-keys">
            <Keys keys={['↵']} />
          </span>
        </div>
        {!question.trim() && (
          <ul className="cp-examples t2">
            <li>What did I decide about the retention purge?</li>
            <li>Where did I leave the checkout flake?</li>
            <li>How much time went to the cost memo this week?</li>
          </ul>
        )}
      </div>
    );
  }

  if (state.status === 'loading') {
    return (
      <div className="cp">
        <div className="cp-q">{state.question}</div>
        <div className="cp-loading t2">
          <span className="spinner" /> Reading your last two weeks…
        </div>
        <div className="cp-skel">
          <div className="skeleton" style={{ width: '94%', height: 10 }} />
          <div className="skeleton" style={{ width: '88%', height: 10 }} />
          <div className="skeleton" style={{ width: '60%', height: 10 }} />
        </div>
      </div>
    );
  }

  if (state.status === 'unavailable') {
    return (
      <div className="cp">
        <div className="cp-q">{state.question}</div>
        <p className="cp-answer t2">Answers need a model. Choose one in Settings → Intelligence; a local model keeps everything on this computer.</p>
      </div>
    );
  }

  if (state.status === 'error') {
    return (
      <div className="cp">
        <div className="cp-q">{state.question}</div>
        <p className="cp-answer t2">Couldn’t get an answer: {state.message}. Press ↵ to try again.</p>
      </div>
    );
  }

  const { response } = state;
  const proposal = response.resume_proposal;
  return (
    <div className="cp">
      <div className="cp-q">{state.question}</div>
      <AnswerText text={response.answer} citations={response.citations} onCite={onCite} />
      {response.evidence_status === 'insufficient' && (
        <p className="cp-note t3">Not much history matched, so this answer may be incomplete.</p>
      )}
      {response.citations.length > 0 && (
        <div className="cp-sources">
          <div className="section-label">Sources</div>
          {response.citations.map((c, i) => {
            const lane = citationLane(c) ?? laneColor(c.label);
            return (
              <button
                key={c.intent_id}
                className={`row cite-row ${i === selectedCitation ? 'is-selected' : ''}`}
                onMouseMove={() => onHoverCitation(i)}
                onClick={() => onCite(c)}
              >
                <span className="cite-num mono">{i + 1}</span>
                <span className="lane-swatch" style={{ background: lane }} />
                <span className="row-label ellipsis">{c.label}</span>
                <span className="row-meta mono">{relativeDay(c.date)}</span>
              </button>
            );
          })}
        </div>
      )}
      {proposal && (
        <div className="cp-actions">
          <button className="btn btn-sm" onClick={() => onResume(proposal.intent_id)}>
            <Icon name="review" size={14} /> Review and resume this work
          </button>
        </div>
      )}
    </div>
  );
}
