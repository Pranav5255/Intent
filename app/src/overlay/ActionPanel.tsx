import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { Icon } from '../components/Icon';
import { Keys } from '../components/ui';
import { fuzzyRank } from '../lib/fuzzy';
import type { ItemAction, ItemActionId } from './commands';

/** ⌘/Ctrl+K panel: actions for the selected item, with its own filter. */
export function ActionPanel({
  title,
  actions,
  onRun,
  onClose,
}: {
  title: string;
  actions: ItemAction[];
  onRun: (id: ItemActionId) => void;
  onClose: () => void;
}) {
  const [filter, setFilter] = useState('');
  const [index, setIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const visible = useMemo(() => fuzzyRank(actions, filter, (a) => a.title), [actions, filter]);

  useEffect(() => inputRef.current?.focus(), []);
  useEffect(() => setIndex(0), [filter]);

  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Escape' || ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k')) {
      e.preventDefault();
      e.stopPropagation();
      onClose();
    } else if (e.key === 'ArrowDown') {
      e.preventDefault();
      setIndex((i) => (i + 1) % Math.max(1, visible.length));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setIndex((i) => (i - 1 + visible.length) % Math.max(1, visible.length));
    } else if (e.key === 'Enter') {
      e.preventDefault();
      e.stopPropagation();
      const action = visible[index];
      if (action) onRun(action.id);
    }
  };

  // Group destructive actions below a hairline, GNOME-menu style.
  const firstDanger = visible.findIndex((a) => a.destructive);

  return (
    <div className="ap" role="menu" aria-label={`Actions for ${title}`} onKeyDown={onKey}>
      <div className="ap-title ellipsis t2">{title}</div>
      <div className="ap-list">
        {visible.map((a, i) => (
          <div key={a.id}>
            {i === firstDanger && i > 0 && <div className="ap-sep" />}
            <button
              role="menuitem"
              className={`ap-item ${i === index ? 'is-selected' : ''} ${a.destructive ? 'is-danger' : ''}`}
              onMouseMove={() => setIndex(i)}
              onClick={() => onRun(a.id)}
            >
              <Icon name={a.icon} size={15} />
              <span className="ellipsis">{a.title}</span>
              {a.keys && <Keys keys={a.keys} />}
            </button>
          </div>
        ))}
        {visible.length === 0 && <div className="ap-empty t3">No matching actions</div>}
      </div>
      <div className="ap-search">
        <input
          ref={inputRef}
          className="ap-input"
          placeholder="Search actions…"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          aria-label="Filter actions"
        />
      </div>
    </div>
  );
}
