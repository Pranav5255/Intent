import { useState, type FormEvent } from 'react';
import type { RetentionPolicy } from '../types';

interface StorageSettingsSectionProps {
  retention: RetentionPolicy | null;
  retentionLabel: string;
  detailedEventCount: number;
  saving: boolean;
  error: string | null;
  onSaveRetention: (policy: RetentionPolicy) => Promise<void>;
  onPurgeDetailed: () => Promise<void>;
  onPurgeRetention: (policy: RetentionPolicy) => Promise<void>;
}

const RETENTION_PRESETS: Array<{ label: string; metadata_days: number | null; detailed_days: number | null }> = [
  { label: 'Keep until deleted', metadata_days: null, detailed_days: null },
  { label: '30 days metadata / 7 days detailed', metadata_days: 30, detailed_days: 7 },
  { label: '90 days metadata / 30 days detailed', metadata_days: 90, detailed_days: 30 },
];

export function StorageSettingsSection({
  retention,
  retentionLabel,
  detailedEventCount,
  saving,
  error,
  onSaveRetention,
  onPurgeDetailed,
  onPurgeRetention,
}: StorageSettingsSectionProps) {
  const [selectedPreset, setSelectedPreset] = useState(0);

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const preset = RETENTION_PRESETS[selectedPreset];
    void onSaveRetention({ metadata_days: preset.metadata_days, detailed_days: preset.detailed_days });
  };

  return (
    <form className="settings-form" onSubmit={submit}>
      <p className="settings-intro">Retention applies locally on this machine. Purge actions cannot be undone.</p>
      <div className="settings-field">
        <span>Current retention policy</span>
        <p className="settings-intro">{retentionLabel}</p>
      </div>
      <div className="settings-field">
        <span>Stored detailed events</span>
        <p className="settings-intro">{detailedEventCount}</p>
      </div>
      <fieldset className="settings-field">
        <legend>Retention preset</legend>
        {RETENTION_PRESETS.map((preset, index) => (
          <label className="settings-toggle" key={preset.label}>
            <input type="radio" name="retention-preset" checked={selectedPreset === index} onChange={() => setSelectedPreset(index)} disabled={saving} />
            <span><strong>{preset.label}</strong></span>
          </label>
        ))}
      </fieldset>
      {error && <p className="settings-error" role="alert">{error}</p>}
      <div className="settings-actions">
        <button className="compact-primary" type="submit" disabled={saving}>{saving ? 'Saving…' : 'Save retention policy'}</button>
        <button className="compact-quiet" type="button" disabled={saving} onClick={() => void onPurgeDetailed()}>Delete detailed data now</button>
        <button className="compact-quiet" type="button" disabled={saving || !retention} onClick={() => retention && void onPurgeRetention(retention)}>Run retention purge</button>
      </div>
    </form>
  );
}
