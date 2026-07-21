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
  productionSettings?: boolean;
  onOpenSettings?: () => void;
}

export const CommandBar = forwardRef<HTMLInputElement, CommandBarProps>(function CommandBar(
  { value, mode, open, expanded, busy, onChange, onKeyDown, onOpen, onClose, productionSettings = false, onOpenSettings },
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
      {open && productionSettings && onOpenSettings && (
        <button className="settings-button button-reset" type="button" onClick={onOpenSettings} aria-label="Open production settings">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M19.14 12.94a7.42 7.42 0 0 0 .05-.94 7.42 7.42 0 0 0-.05-.94l2.03-1.58-1.92-3.32-2.39.96a7.06 7.06 0 0 0-1.63-.95L14.87 3h-3.84l-.36 2.13c-.58.24-1.12.55-1.63.95l-2.39-.96-1.92 3.32 2.03 1.58a7.42 7.42 0 0 0-.05.94c0 .32.02.63.05.94l-2.03 1.58 1.92 3.32 2.39-.96c.5.4 1.05.72 1.63.95l.36 2.13h3.84l.36-2.13a7.06 7.06 0 0 0 1.63-.95l2.39.96 1.92-3.32-2.03-1.58ZM12.95 15.5a3.5 3.5 0 1 1 0-7 3.5 3.5 0 0 1 0 7Z" /></svg>
        </button>
      )}
      {open ? (
        <button className="key-hint button-reset" type="button" onClick={onClose} aria-label="Close Intent">
          Esc
        </button>
      ) : (
        <span className="key-hint" aria-hidden="true">Ctrl Space</span>
      )}
    </div>
  );
});
