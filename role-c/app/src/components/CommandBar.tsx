import { forwardRef, type KeyboardEvent } from 'react';
import { modeLabel } from '../lib/parseMode';
import type { CommandMode } from '../types';

interface CommandBarProps {
  value: string;
  mode: CommandMode;
  open: boolean;
  expanded: boolean;
  busy: boolean;
  onChange: (value: string) => void;
  onKeyDown: (event: KeyboardEvent<HTMLInputElement>) => void;
  onOpen: () => void;
  onClose: () => void;
}

export const CommandBar = forwardRef<HTMLInputElement, CommandBarProps>(function CommandBar(
  { value, mode, open, expanded, busy, onChange, onKeyDown, onOpen, onClose },
  ref,
) {
  return (
    <div className={`command-bar glass ${open ? 'is-open' : ''}`}>
      <span className="app-glyph" aria-hidden="true">
        <svg viewBox="0 0 24 24"><path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm-1 15h2v-6h-2v6zm0-8h2V7h-2v2z" /></svg>
      </span>
      <span className="command-mode" aria-hidden="true">{modeLabel(mode)}</span>
      <input
        ref={ref}
        value={value}
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={onKeyDown}
        onFocus={onOpen}
        placeholder="Search yesterday’s work"
        aria-label="Search your saved work. Start with a question mark to ask Copilot."
        aria-expanded={expanded}
        aria-controls="intent-dashboard"
        autoComplete="off"
        spellCheck="false"
      />
      {busy && <span className="command-spinner" aria-label="Loading" />}
      {open ? (
        <button className="key-hint button-reset" type="button" onClick={onClose} aria-label="Close Intent OS">
          Esc
        </button>
      ) : (
        <span className="key-hint" aria-hidden="true">Ctrl Space</span>
      )}
    </div>
  );
});
