import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import { Icon } from '../components/Icon';
import { Empty, Keys, Snippet } from '../components/ui';
import { api, ApiError } from '../lib/api';
import { daysAgoIso, relativeDay, todayIso } from '../lib/format';
import { errorText } from '../lib/store';
import type { SearchHit } from '../lib/types';
import { CopilotPanel, type CopilotState } from '../overlay/CopilotAnswer';
import type { MainActions } from './MainWindow';

type Span = 14 | 30 | 0;

export function SearchView({ actions }: { actions: MainActions }) {
  const [q, setQ] = useState('');
  const [win, setWin] = useState<Span>(14);
  const [hits, setHits] = useState<SearchHit[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [index, setIndex] = useState(0);
  const [copilot, setCopilot] = useState<CopilotState>({ status: 'idle' });
  const inputRef = useRef<HTMLInputElement>(null);
  const asking = q.trimStart().startsWith('?');
  const text = asking ? q.trimStart().slice(1).trim() : q.trim();

  useEffect(() => inputRef.current?.focus(), []);

  useEffect(() => {
    if (asking || text.length < 2) {
      setHits(null);
      return;
    }
    let live = true;
    setBusy(true);
    const t = window.setTimeout(() => {
      api
        .search(text, { limit: 30, date_from: win ? daysAgoIso(win) : undefined })
        .then((r) => live && (setHits(r), setIndex(0)))
        .catch(() => live && setHits([]))
        .finally(() => live && setBusy(false));
    }, 140);
    return () => {
      live = false;
      window.clearTimeout(t);
    };
  }, [text, win, asking]);

  const ask = async (question: string) => {
    setCopilot({ status: 'loading', question });
    try {
      const response = await api.copilot({ question, mode: 'auto', date_from: win ? daysAgoIso(win) : null, date_to: todayIso() });
      setCopilot({ status: 'done', question, response });
    } catch (reason) {
      if (reason instanceof ApiError && reason.status === 503) setCopilot({ status: 'unavailable', question });
      else setCopilot({ status: 'error', question, message: errorText(reason) });
    }
  };

  const onKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'ArrowDown' && hits?.length) {
      e.preventDefault();
      setIndex((i) => Math.min(hits.length - 1, i + 1));
    } else if (e.key === 'ArrowUp' && hits?.length) {
      e.preventDefault();
      setIndex((i) => Math.max(0, i - 1));
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (asking && text) void ask(text);
      else if (hits?.[index]) actions.openIntent(hits[index]!.id);
    } else if (e.key === 'Escape') {
      setQ('');
      setCopilot({ status: 'idle' });
    }
  };

  return (
    <div className="page">
      <header className="page-head">
        <h1 className="page-title display">Search</h1>
      </header>
      <div className="search-bar">
        <Icon name={asking ? 'ask' : 'search'} size={16} className="search-icon" />
        <input
          ref={inputRef}
          className="search-input"
          placeholder="Search threads, files, tabs and commands — or start with ? to ask"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={onKey}
          spellCheck={false}
          aria-label="Search"
        />
        {busy && <span className="spinner" />}
        <div className="segmented" role="group" aria-label="Time range">
          {([14, 30, 0] as Span[]).map((w) => (
            <button key={w} aria-pressed={win === w} onClick={() => setWin(w)}>
              {w ? `${w} days` : 'All'}
            </button>
          ))}
        </div>
      </div>

      {asking ? (
        <div className="search-answer boxed">
          <CopilotPanel
            state={copilot}
            question={text}
            selectedCitation={-1}
            onHoverCitation={() => undefined}
            onCite={(c) => actions.openIntent(c.intent_id)}
            onResume={(id) => void api.intent(id).then((i) => actions.review(i))}
            citationLane={() => null}
          />
        </div>
      ) : hits === null ? (
        <div className="search-tips t2">
          <p>
            Search is hybrid: exact words and similar meaning both count, so “payments” finds the Stripe webhook review.
          </p>
          <p className="search-tip-keys">
            <Keys keys={['↑', '↓']} /> move <Keys keys={['↵']} /> open <Keys keys={['?']} /> ask a question
          </p>
        </div>
      ) : hits.length === 0 && !busy ? (
        <Empty icon="search" title={`Nothing matched “${text}”${win ? ` in the last ${win} days` : ''}`} text="Try fewer words, a wider range, or ask in plain language.">
          <button className="btn btn-sm" onClick={() => (setQ(`? ${text}`), void ask(text))}>
            Ask instead <Keys keys={['?']} />
          </button>
        </Empty>
      ) : (
        <div className="list boxed search-results" role="listbox">
          {hits.map((h, i) => (
            <button
              key={h.id}
              role="option"
              aria-selected={i === index}
              className={`lrow lrow-tall ${i === index ? 'is-selected' : ''}`}
              onMouseMove={() => setIndex(i)}
              onClick={() => actions.openIntent(h.id)}
            >
              <span className="lrow-stack">
                <span className="lrow-label ellipsis">{h.label}</span>
                <span className="lrow-snippet">
                  <Snippet text={h.highlight_snippet || h.summary} />
                </span>
              </span>
              <span className="lrow-meta mono">{relativeDay(h.date)}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
