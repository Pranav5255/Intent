import { useEffect } from 'react';
import { Icon } from '../components/Icon';
import { Keys } from '../components/ui';
import { IS_MAC } from '../lib/env';

const GROUPS: Array<{ title: string; items: Array<[string, string[]]> }> = [
  {
    title: 'Palette',
    items: [
      ['Open Intent from anywhere', IS_MAC ? ['⌥', 'Space'] : ['Super', 'Space']],
      ['Resume the selected thread', ['↵']],
      ['Review before reopening', ['mod', '↵']],
      ['Actions for the selected item', ['mod', 'K']],
      ['Show or hide the preview', ['mod', '.']],
      ['Copy summary', ['mod', 'C']],
      ['Copy context as Markdown', ['mod', '⇧', 'C']],
      ['Close', ['Esc']],
    ],
  },
  {
    title: 'Typing in the palette',
    items: [
      ['Ask a question', ['?']],
      ['Run a command', ['>']],
      ['Scope to a project', ['@']],
      ['Commit the project scope', ['Tab']],
      ['Remove the scope', ['⌫']],
    ],
  },
  {
    title: 'Main window',
    items: [
      ['Today', ['mod', '1']],
      ['Timeline', ['mod', '2']],
      ['Search', ['mod', '3']],
      ['Settings', ['mod', ',']],
      ['Search settings', ['mod', 'F']],
    ],
  },
];

/** AdwShortcutsDialog-like sheet. */
export function ShortcutsSheet({ onClose }: { onClose: () => void }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        onClose();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="sheet" role="dialog" aria-label="Keyboard shortcuts">
        <header className="sheet-head">
          <h2 className="display">Keyboard shortcuts</h2>
          <button className="btn btn-quiet btn-icon btn-sm" onClick={onClose} aria-label="Close">
            <Icon name="x" size={14} />
          </button>
        </header>
        <div className="sheet-body">
          {GROUPS.map((g) => (
            <div key={g.title} className="group">
              <div className="group-head">
                <div className="group-title">{g.title}</div>
              </div>
              <div className="boxed">
                {g.items.map(([label, keys]) => (
                  <div key={label} className="sc-row">
                    <span>{label}</span>
                    <Keys keys={keys} />
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
