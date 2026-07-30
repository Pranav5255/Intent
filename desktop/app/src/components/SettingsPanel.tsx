import { useState } from 'react';
import type {
  DetailedCaptureSettings,
  DetailedCaptureSettingsUpdate,
  IntelligencePreviewSample,
  LLMSettings,
  LLMSettingsUpdate,
  RetentionPolicy,
  SettingsTab,
} from '../types';
import { CaptureSettingsSection } from './CaptureSettingsSection';
import { IntelligenceSettingsSection } from './IntelligenceSettingsSection';
import { StorageSettingsSection } from './StorageSettingsSection';

interface SettingsPanelProps {
  activeTab: SettingsTab;
  onTabChange: (tab: SettingsTab) => void;
  settings: LLMSettings | null;
  capture: DetailedCaptureSettings | null;
  retention: RetentionPolicy | null;
  filesystemAccessible: boolean;
  detailedEventCount: number;
  preview: IntelligencePreviewSample | null;
  loading: boolean;
  saving: boolean;
  error: string | null;
  electronAvailable: boolean;
  onClose: () => void;
  onSaveLlm: (update: LLMSettingsUpdate) => void;
  onSaveCapture: (update: DetailedCaptureSettingsUpdate, filesystemAccessible?: boolean) => Promise<void>;
  onSaveRetention: (policy: RetentionPolicy) => Promise<void>;
  onPurgeDetailed: () => Promise<void>;
  onPurgeRetention: (policy: RetentionPolicy) => Promise<void>;
  onLoadPreview: () => Promise<void>;
  onAttachGeminiCredentials: () => Promise<void>;
  onClearGeminiCredentials: () => Promise<void>;
}

const TABS: Array<{ id: SettingsTab; label: string }> = [
  { id: 'capture', label: 'Capture' },
  { id: 'intelligence', label: 'Intelligence' },
  { id: 'storage', label: 'Storage' },
];

export function SettingsPanel({
  activeTab,
  onTabChange,
  settings,
  capture,
  retention,
  filesystemAccessible,
  detailedEventCount,
  preview,
  loading,
  saving,
  error,
  electronAvailable,
  onClose,
  onSaveLlm,
  onSaveCapture,
  onSaveRetention,
  onPurgeDetailed,
  onPurgeRetention,
  onLoadPreview,
  onAttachGeminiCredentials,
  onClearGeminiCredentials,
}: SettingsPanelProps) {
  const [captureError, setCaptureError] = useState<string | null>(null);
  const [storageError, setStorageError] = useState<string | null>(null);

  const saveCapture = async (update: DetailedCaptureSettingsUpdate, allAccessible?: boolean) => {
    setCaptureError(null);
    try {
      await onSaveCapture(update, allAccessible);
    } catch (reason) {
      setCaptureError(reason instanceof Error ? reason.message : 'Could not save capture settings.');
      throw reason;
    }
  };

  const saveRetention = async (policy: RetentionPolicy) => {
    setStorageError(null);
    try {
      await onSaveRetention(policy);
    } catch (reason) {
      setStorageError(reason instanceof Error ? reason.message : 'Could not save retention policy.');
      throw reason;
    }
  };

  return (
    <section className="settings-panel glass" aria-labelledby="settings-title">
      <div className="settings-heading">
        <div>
          <span className="section-kicker">Local settings</span>
          <h2 id="settings-title">Capture & intelligence</h2>
        </div>
        <button type="button" className="close-button" onClick={onClose} aria-label="Close settings">×</button>
      </div>
      <div className="settings-tabs" role="tablist" aria-label="Settings sections">
        {TABS.map((tab) => (
          <button
            key={tab.id}
            type="button"
            role="tab"
            aria-selected={activeTab === tab.id}
            className={activeTab === tab.id ? 'is-active' : ''}
            onClick={() => onTabChange(tab.id)}
          >
            {tab.label}
          </button>
        ))}
      </div>
      {loading ? <div className="loading-state"><span className="loader" />Loading local settings…</div> : (
        <>
          {activeTab === 'capture' && (
            <CaptureSettingsSection
              capture={capture}
              filesystemAccessible={filesystemAccessible}
              saving={saving}
              error={captureError}
              onSave={saveCapture}
            />
          )}
          {activeTab === 'intelligence' && (
            <IntelligenceSettingsSection
              settings={settings}
              preview={preview}
              loading={loading}
              saving={saving}
              error={error}
              electronAvailable={electronAvailable}
              onSave={onSaveLlm}
              onAttachGeminiCredentials={onAttachGeminiCredentials}
              onClearGeminiCredentials={onClearGeminiCredentials}
              onLoadPreview={onLoadPreview}
            />
          )}
          {activeTab === 'storage' && (
            <StorageSettingsSection
              retention={retention}
              retentionLabel={capture?.retention ?? 'indefinite'}
              detailedEventCount={detailedEventCount}
              saving={saving}
              error={storageError}
              onSaveRetention={saveRetention}
              onPurgeDetailed={onPurgeDetailed}
              onPurgeRetention={onPurgeRetention}
            />
          )}
        </>
      )}
    </section>
  );
}
