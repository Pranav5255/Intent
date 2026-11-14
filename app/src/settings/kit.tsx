import { createContext, useContext, type ReactNode } from 'react';
import { Icon, type IconName } from '../components/Icon';

/** Current settings search query; rows that don't match render nothing. */
export const SettingsQuery = createContext('');

function matches(q: string, parts: Array<string | undefined>): boolean {
  if (!q.trim()) return true;
  const hay = parts.filter(Boolean).join(' ').toLowerCase();
  return q
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((t) => hay.includes(t));
}

export function Section({ id, title, icon, children }: { id: string; title: string; icon: IconName; children: ReactNode }) {
  return (
    <section className="set-section" id={`settings-${id}`} data-section={id}>
      <h2 className="set-title display">
        <Icon name={icon} size={16} />
        {title}
      </h2>
      {children}
    </section>
  );
}

export function Group({
  title,
  description,
  action,
  children,
}: {
  title?: string;
  description?: ReactNode;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="group">
      {(title || description) && (
        <div className="group-head">
          <div>
            {title && <div className="group-title">{title}</div>}
            {description && <div className="group-desc">{description}</div>}
          </div>
          {action}
        </div>
      )}
      <div className="boxed">{children}</div>
    </div>
  );
}

/** One row, one control (GNOME boxed-list convention). */
export function Row({
  title,
  subtitle,
  keywords,
  icon,
  children,
  mono,
  danger,
  stack,
  lead,
}: {
  title: ReactNode;
  subtitle?: ReactNode;
  keywords?: string;
  icon?: IconName;
  children?: ReactNode;
  mono?: boolean;
  danger?: boolean;
  stack?: boolean;
  lead?: ReactNode;
}) {
  const q = useContext(SettingsQuery);
  const text = [typeof title === 'string' ? title : '', typeof subtitle === 'string' ? subtitle : '', keywords];
  if (!matches(q, text)) return null;
  return (
    <div className={`prow ${danger ? 'is-danger' : ''} ${stack ? 'prow-stack' : ''}`}>
      {lead}
      {icon && <Icon name={icon} className="prow-icon" />}
      <div className="prow-body">
        <div className="prow-title">{title}</div>
        {subtitle && <div className={`prow-sub ${mono ? 'mono' : ''}`}>{subtitle}</div>}
      </div>
      {children && <div className="prow-control">{children}</div>}
    </div>
  );
}
