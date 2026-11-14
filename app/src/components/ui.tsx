import { useEffect, useRef, type ReactNode } from 'react';
import { MOD } from '../lib/env';
import { laneColor } from '../lib/intent';
import { dismissToast, useToasts, type CaptureSummary } from '../lib/store';
import { Icon, type IconName } from './Icon';

/** Keycaps. `mod` renders ⌘ on macOS and Ctrl elsewhere. */
export function Keys({ keys }: { keys: string[] }) {
  return (
    <span className="kbd-group">
      {keys.map((k, i) => (
        <kbd className="kbd" key={i}>
          {k === 'mod' ? MOD : k}
        </kbd>
      ))}
    </span>
  );
}

export function Switch({
  checked,
  onChange,
  disabled,
  label,
}: {
  checked: boolean;
  onChange: (next: boolean) => void;
  disabled?: boolean;
  label: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      className="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
    />
  );
}

export function Check({ checked, onChange, label }: { checked: boolean; onChange: (next: boolean) => void; label: string }) {
  return (
    <button type="button" role="checkbox" className="check" aria-checked={checked} aria-label={label} onClick={() => onChange(!checked)}>
      <Icon name="check" size={12} />
    </button>
  );
}

export function StatusDot({ tone }: { tone: CaptureSummary['tone'] }) {
  return <span className={`dot dot-${tone === 'idle' ? 'idle' : tone}`} aria-hidden="true" />;
}

export function Project({ name }: { name: string }) {
  return (
    <span className="project">
      <span className="lane-dot" style={{ background: laneColor(name) }} />
      {name}
    </span>
  );
}

export function Empty({
  icon,
  title,
  text,
  children,
}: {
  icon: IconName;
  title: string;
  text?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <div className="empty">
      <Icon name={icon} size={24} className="empty-icon" />
      <div className="empty-title">{title}</div>
      {text && <div className="empty-text">{text}</div>}
      {children}
    </div>
  );
}

export function Toasts() {
  const toasts = useToasts();
  return (
    <div className="toasts" role="status" aria-live="polite">
      {toasts.map((t) => (
        <div key={t.id} className={`toast is-${t.tone}`}>
          <span className="toast-icon">
            {t.tone === 'progress' ? (
              <span className="spinner" />
            ) : (
              <Icon name={t.tone === 'success' ? 'check' : t.tone === 'error' ? 'alert' : 'info'} size={15} />
            )}
          </span>
          <div className="toast-body">
            <div className="toast-text">{t.text}</div>
            {t.detail && <div className="toast-detail">{t.detail}</div>}
          </div>
          {t.action && (
            <button
              className="btn btn-quiet btn-sm"
              onClick={() => {
                t.action?.run();
                dismissToast(t.id);
              }}
            >
              {t.action.label}
            </button>
          )}
        </div>
      ))}
    </div>
  );
}

export interface ConfirmSpec {
  title: string;
  text: ReactNode;
  facts?: Array<[string, ReactNode]>;
  confirm: string;
  destructive?: boolean;
  onConfirm: () => void | Promise<void>;
}

export function ConfirmDialog({ spec, onClose }: { spec: ConfirmSpec; onClose: () => void }) {
  const cancelRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    // Safe default: focus lands on Cancel, never on the destructive action.
    cancelRef.current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [onClose]);

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="dialog" role="alertdialog" aria-modal="true" aria-labelledby="confirm-title">
        <div className="dialog-body">
          <h2 id="confirm-title" className="dialog-title display">
            {spec.title}
          </h2>
          <div className="dialog-text">{spec.text}</div>
          {spec.facts && spec.facts.length > 0 && (
            <div className="dialog-facts">
              {spec.facts.map(([k, v]) => (
                <div key={k}>
                  <span className="t2">{k}</span>
                  <span className="num">{v}</span>
                </div>
              ))}
            </div>
          )}
        </div>
        <div className="dialog-foot">
          <button ref={cancelRef} className="btn btn-quiet" onClick={onClose}>
            Cancel
          </button>
          <button
            className={`btn ${spec.destructive ? 'btn-danger-solid' : 'btn-primary'}`}
            onClick={() => {
              void spec.onConfirm();
              onClose();
            }}
          >
            {spec.confirm}
          </button>
        </div>
      </div>
    </div>
  );
}

/** Render `**term**` highlight markers from search snippets as <mark>. */
export function Snippet({ text }: { text: string }) {
  const parts = text.split(/\*\*(.+?)\*\*/g);
  return (
    <>
      {parts.map((part, i) => (i % 2 === 1 ? <mark key={i}>{part}</mark> : <span key={i}>{part}</span>))}
    </>
  );
}
