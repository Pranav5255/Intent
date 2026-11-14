import { Icon } from '../components/Icon';
import { RestoreChecklist } from '../components/RestoreChecklist';
import { Project } from '../components/ui';
import { duration, range, relativeDay } from '../lib/format';
import { laneFor, projectOf, whereLeftOff } from '../lib/intent';
import type { RestoreSelection } from '../lib/restore';
import type { Intent } from '../lib/types';
import type { Command } from './commands';

export function LeftOffLine({ intent }: { intent: Intent }) {
  const left = whereLeftOff(intent);
  if (!left) return null;
  const icon = left.kind === 'file' ? 'code' : left.kind === 'tab' ? 'globe' : 'terminal';
  return (
    <div className="leftoff">
      <Icon name={icon} size={14} />
      <span className={`ellipsis ${left.kind === 'tab' ? '' : 'mono'}`}>{left.kind === 'cmd' ? `$ ${left.text}` : left.text}</span>
    </div>
  );
}

export function IntentPreview({
  intent,
  selection,
  onSelection,
}: {
  intent: Intent;
  selection: RestoreSelection;
  onSelection: (s: RestoreSelection) => void;
}) {
  const project = projectOf(intent);
  return (
    <div className="pv">
      <div className="pv-head">
        <span className="lane-swatch" style={{ background: laneFor(intent), height: 16 }} />
        <h3 className="pv-title display">{intent.label}</h3>
      </div>
      <div className="pv-meta">
        {project && <Project name={project} />}
        <span className="mono t3">
          {relativeDay(intent.date)} · {range(intent.start_ts, intent.end_ts)}
        </span>
        <span className="mono t3">{duration(intent.stats.duration_seconds)} active</span>
      </div>
      <div className="pv-section">
        <div className="pv-label">Where you left off</div>
        <LeftOffLine intent={intent} />
      </div>
      <p className="pv-summary">{intent.summary}</p>
      <div className="pv-section pv-restore">
        <div className="pv-label">Resume reopens</div>
        <RestoreChecklist compact payload={intent.resume_payload} insights={intent.insights} selection={selection} onChange={onSelection} />
      </div>
    </div>
  );
}

export function CommandPreview({ command }: { command: Command }) {
  const explain: Record<string, string> = {
    'pause-15m': 'Nothing is recorded from any source while paused. Capture resumes on its own.',
    'pause-1h': 'Nothing is recorded from any source while paused. Capture resumes on its own.',
    'pause-tomorrow': 'Nothing is recorded until 08:00 tomorrow. You can resume early from the tray.',
    'resume-capture': 'Start recording activity again from all enabled sources.',
    'run-pipeline': 'Groups today’s activity into threads now instead of waiting for the next scheduled run.',
    'open-today': 'Today’s digest and the work you can pick back up.',
    'open-timeline': 'Your day as lanes, one per thread, with idle time collapsed.',
    'open-search': 'Search every thread from the last two weeks.',
    'open-settings': 'Capture sources, privacy, the model Intent uses, and storage.',
    ask: 'Ask a question about your recent work. Answers cite the threads they come from.',
  };
  return (
    <div className="pv pv-command">
      <span className="pv-command-icon">
        <Icon name={command.icon} size={18} />
      </span>
      <h3 className="pv-title display">{command.title}</h3>
      <p className="pv-summary">{explain[command.id] ?? command.subtitle}</p>
    </div>
  );
}

export function PreviewLoading() {
  return (
    <div className="pv">
      <div className="skeleton" style={{ width: '70%', height: 14 }} />
      <div className="skeleton" style={{ width: '45%', height: 10, marginTop: 10 }} />
      <div className="skeleton" style={{ width: '90%', height: 10, marginTop: 22 }} />
      <div className="skeleton" style={{ width: '80%', height: 10, marginTop: 8 }} />
    </div>
  );
}
