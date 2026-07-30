import { useEffect, useState } from 'react';
import type { IntelligencePreviewSample, LLMSettings, LLMSettingsUpdate } from '../types';
import { CopilotSettingsSection } from './CopilotSettingsSection';

interface IntelligenceSettingsSectionProps {
  settings: LLMSettings | null;
  preview: IntelligencePreviewSample | null;
  loading: boolean;
  saving: boolean;
  error: string | null;
  electronAvailable: boolean;
  onSave: (update: LLMSettingsUpdate) => void;
  onAttachGeminiCredentials: () => Promise<void>;
  onClearGeminiCredentials: () => Promise<void>;
  onLoadPreview: () => Promise<void>;
}

export function IntelligenceSettingsSection({
  settings,
  preview,
  loading,
  saving,
  error,
  electronAvailable,
  onSave,
  onAttachGeminiCredentials,
  onClearGeminiCredentials,
  onLoadPreview,
}: IntelligenceSettingsSectionProps) {
  const [semanticCluster, setSemanticCluster] = useState(false);
  const [semanticContent, setSemanticContent] = useState(false);
  const [semanticFullCapture, setSemanticFullCapture] = useState(false);

  useEffect(() => {
    if (!settings) return;
    setSemanticCluster(settings.semantic_cluster_enabled);
    setSemanticContent(settings.semantic_content_consent);
    setSemanticFullCapture(settings.semantic_full_capture_consent);
  }, [settings]);

  const saveSemantic = () => {
    if (!settings) return;
    onSave({
      provider: settings.provider,
      enable_copilot: settings.copilot_enabled,
      enable_semantic_cluster: semanticCluster,
      enable_semantic_content_consent: semanticContent,
      enable_semantic_full_capture_consent: semanticFullCapture,
      model: settings.model || undefined,
    });
  };

  return (
    <div className="settings-form">
      <section className="settings-subsection">
        <h3>What may leave this device</h3>
        <p className="settings-intro">
          Copilot and smarter grouping send privacy-bounded summaries to your configured provider.
          Tab metadata stays local unless you enable the options below.
        </p>
        <ul className="settings-disclosure">
          <li>Domain names and file basenames — not full paths or URL query params</li>
          <li>Activity counts, durations, and optional semantic topics</li>
          <li>Up to 180 characters of page excerpts when Actions + excerpts capture is on</li>
          <li>Never sent: passwords, blocked domains, API keys, full page HTML</li>
        </ul>
        <label className="settings-toggle">
          <input type="checkbox" checked={semanticContent} onChange={(event) => setSemanticContent(event.target.checked)} disabled={saving || loading} />
          <span><strong>Share activity summaries with LLM</strong><small>Required for smarter grouping and richer labels.</small></span>
        </label>
        <label className="settings-toggle">
          <input type="checkbox" checked={semanticCluster} onChange={(event) => setSemanticCluster(event.target.checked)} disabled={saving || loading || !semanticContent} />
          <span><strong>Smarter grouping</strong><small>Let the provider refine session clusters using bounded timelines.</small></span>
        </label>
        <label className="settings-toggle">
          <input type="checkbox" checked={semanticFullCapture} onChange={(event) => setSemanticFullCapture(event.target.checked)} disabled={saving || loading || !semanticContent} />
          <span><strong>Full activity replay to LLM (advanced)</strong><small>Sends complete captured event fields under explicit consent.</small></span>
        </label>
        <div className="settings-actions">
          <button className="compact-quiet" type="button" onClick={() => void onLoadPreview()} disabled={loading || saving}>Preview sample packet</button>
          <button className="compact-primary" type="button" onClick={saveSemantic} disabled={saving || loading || !settings}>{saving ? 'Saving…' : 'Save intelligence settings'}</button>
        </div>
        {preview && (
          <pre className="settings-preview">{JSON.stringify(preview, null, 2)}</pre>
        )}
      </section>
      <CopilotSettingsSection
        settings={settings}
        loading={loading}
        saving={saving}
        error={error}
        electronAvailable={electronAvailable}
        onSave={onSave}
        onAttachGeminiCredentials={onAttachGeminiCredentials}
        onClearGeminiCredentials={onClearGeminiCredentials}
      />
    </div>
  );
}
