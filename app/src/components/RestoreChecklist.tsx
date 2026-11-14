import { basename, pathOf, tildify } from '../lib/format';
import type { RestoreSelection } from '../lib/restore';
import type { InsightRow, ResumePayload } from '../lib/types';
import { Icon } from './Icon';
import { Check } from './ui';

interface Props {
  payload: ResumePayload;
  insights?: { editor: InsightRow[]; browser: InsightRow[] };
  selection: RestoreSelection;
  onChange: (next: RestoreSelection) => void;
  compact?: boolean;
}

function toggle(list: string[], item: string, on: boolean): string[] {
  return on ? (list.includes(item) ? list : [...list, item]) : list.filter((x) => x !== item);
}

export function RestoreChecklist({ payload, insights, selection, onChange, compact }: Props) {
  const lineOf = (file: string) => {
    const row = insights?.editor.find((e) => e.file === file);
    return typeof row?.line === 'number' ? `:${row.line}` : '';
  };
  const titleOf = (url: string) => {
    const row = insights?.browser.find((b) => b.url === url);
    return typeof row?.title === 'string' ? row.title : null;
  };
  const cwd = payload.shell.cwd;
  const lastCmd = payload.shell.last_cmd;
  const empty = payload.files.length === 0 && payload.urls.length === 0 && !cwd;

  if (empty) {
    return <p className="rc-empty t2">Nothing was saved that can be reopened for this work.</p>;
  }

  return (
    <div className={`rc ${compact ? 'is-compact' : ''}`}>
      {payload.files.length > 0 && (
        <section className="rc-group">
          <header className="rc-head">
            <span>Files</span>
            <span className="num t3">{payload.files.length}</span>
          </header>
          {payload.files.map((file) => {
            const on = selection.files.includes(file);
            return (
              <label key={file} className={`rc-row ${on ? '' : 'is-off'}`}>
                <Check
                  checked={on}
                  label={`Reopen ${basename(file)}`}
                  onChange={(v) => onChange({ ...selection, files: toggle(selection.files, file, v) })}
                />
                <Icon name="code" className="rc-icon" />
                <span className="rc-main ellipsis">
                  {basename(file)}
                  <span className="t3 num">{lineOf(file)}</span>
                </span>
                <span className="rc-detail mono ellipsis">{tildify(file).replace(/\/[^/]+$/, '')}</span>
              </label>
            );
          })}
        </section>
      )}
      {payload.urls.length > 0 && (
        <section className="rc-group">
          <header className="rc-head">
            <span>Tabs</span>
            <span className="num t3">{payload.urls.length}</span>
          </header>
          {payload.urls.map((url) => {
            const on = selection.urls.includes(url);
            const title = titleOf(url);
            return (
              <label key={url} className={`rc-row ${on ? '' : 'is-off'}`}>
                <Check
                  checked={on}
                  label={`Reopen ${title ?? url}`}
                  onChange={(v) => onChange({ ...selection, urls: toggle(selection.urls, url, v) })}
                />
                <Icon name="globe" className="rc-icon" />
                <span className="rc-main ellipsis">{title ?? pathOf(url)}</span>
                <span className="rc-detail mono ellipsis">{pathOf(url)}</span>
              </label>
            );
          })}
        </section>
      )}
      {cwd && (
        <section className="rc-group">
          <header className="rc-head">
            <span>Terminal</span>
          </header>
          <label className={`rc-row ${selection.terminal ? '' : 'is-off'}`}>
            <Check
              checked={selection.terminal}
              label="Open a terminal"
              onChange={(v) => onChange({ ...selection, terminal: v })}
            />
            <Icon name="terminal" className="rc-icon" />
            <span className="rc-main ellipsis">
              Open a terminal in <span className="mono rc-path">{tildify(cwd)}</span>
            </span>
          </label>
          {lastCmd && (
            <div className="rc-cmd">
              <code className="mono ellipsis">$ {lastCmd}</code>
              <span className="rc-cmd-note">
                <Icon name="lock" size={12} />
                Last command, for reference. Intent never runs it.
              </span>
            </div>
          )}
        </section>
      )}
    </div>
  );
}
