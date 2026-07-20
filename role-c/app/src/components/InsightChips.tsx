import type { Intent } from '../types';

export function InsightChips({ intent }: { intent: Intent }) {
  const chips: Array<{ kind: 'file' | 'domain' | 'failure'; label: string }> = [];
  const file = intent.insights.editor[0]?.file;
  const domain = intent.insights.browser[0]?.domain;
  const shell = intent.insights.shell.find((item) => (item.count ?? 0) > 0 && (item.exit_code ?? 0) !== 0);
  if (file) chips.push({ kind: 'file', label: file.split('/').pop() ?? file });
  if (domain) chips.push({ kind: 'domain', label: domain });
  if (shell?.command_family) chips.push({ kind: 'failure', label: `${shell.command_family} failed` });
  if (!chips.length) return null;

  return (
    <div className="insight-chips" aria-label="Session insights">
      {chips.slice(0, 3).map((chip) => <span className={`chip chip-${chip.kind}`} key={`${chip.kind}-${chip.label}`}>{chip.label}</span>)}
    </div>
  );
}
