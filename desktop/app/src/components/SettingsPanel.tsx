import { useEffect, useState, type FormEvent } from 'react';
import type { LLMProvider, LLMSettings, LLMSettingsUpdate } from '../types';

interface SettingsPanelProps {
  settings: LLMSettings | null;
  loading: boolean;
  saving: boolean;
  error: string | null;
  electronAvailable: boolean;
  onClose: () => void;
  onSave: (update: LLMSettingsUpdate) => void;
  onAttachGeminiCredentials: () => Promise<void>;
  onClearGeminiCredentials: () => Promise<void>;
}

interface FormState {
  provider: LLMProvider;
  apiKey: string;
  model: string;
  enabled: boolean;
  groqBaseUrl: string;
  googleCloudProject: string;
  googleCloudLocation: string;
  bedrockRegion: string;
  bedrockProfile: string;
}

const PROVIDERS: Array<{ value: LLMProvider; label: string; description: string }> = [
  { value: 'openai', label: 'OpenAI', description: 'Responses API' },
  { value: 'groq', label: 'Groq', description: 'OpenAI-compatible API' },
  { value: 'gemini', label: 'Google Gemini', description: 'Vertex AI service account' },
  { value: 'bedrock', label: 'Amazon Bedrock', description: 'Bedrock Converse API' },
];

function formFromSettings(settings: LLMSettings | null): FormState {
  return {
    provider: settings?.provider ?? 'gemini',
    apiKey: '',
    model: settings?.model ?? '',
    enabled: settings?.copilot_enabled ?? true,
    groqBaseUrl: settings?.groq_base_url ?? 'https://api.groq.com/openai/v1',
    googleCloudProject: settings?.google_cloud_project ?? '',
    googleCloudLocation: settings?.google_cloud_location ?? 'us-central1',
    bedrockRegion: settings?.bedrock_region ?? '',
    bedrockProfile: settings?.bedrock_profile ?? '',
  };
}

export function SettingsPanel({
  settings,
  loading,
  saving,
  error,
  electronAvailable,
  onClose,
  onSave,
  onAttachGeminiCredentials,
  onClearGeminiCredentials,
}: SettingsPanelProps) {
  const [form, setForm] = useState<FormState>(() => formFromSettings(settings));
  const [credentialBusy, setCredentialBusy] = useState(false);

  useEffect(() => {
    if (settings) setForm(formFromSettings(settings));
  }, [settings]);

  const update = <Key extends keyof FormState>(key: Key, value: FormState[Key]) => {
    setForm((current) => ({ ...current, [key]: value }));
  };
  const provider = PROVIDERS.find((item) => item.value === form.provider) ?? PROVIDERS[0];
  const keyConfigured = settings?.provider === form.provider && settings.api_key_configured;
  const credentialsConfigured = settings?.provider === form.provider && settings.credentials_configured;
  const keyLabel = form.provider === 'bedrock'
    ? 'Amazon Bedrock API key'
    : form.provider === 'groq'
      ? 'Groq API key'
      : 'OpenAI API key';

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const request: LLMSettingsUpdate = {
      provider: form.provider,
      model: form.model.trim() || undefined,
      enable_copilot: form.enabled,
    };
    if (form.provider !== 'gemini' && form.apiKey.trim()) request.api_key = form.apiKey.trim();
    if (form.provider === 'groq') request.groq_base_url = form.groqBaseUrl.trim();
    if (form.provider === 'gemini') {
      request.google_cloud_project = form.googleCloudProject.trim();
      request.google_cloud_location = form.googleCloudLocation.trim();
    }
    if (form.provider === 'bedrock') {
      request.bedrock_region = form.bedrockRegion.trim();
      request.bedrock_profile = form.bedrockProfile.trim();
    }
    onSave(request);
  };

  const removeSavedKey = () => {
    onSave({
      provider: form.provider,
      clear_api_key: true,
      model: form.model.trim() || undefined,
      enable_copilot: form.enabled,
      ...(form.provider === 'groq' ? { groq_base_url: form.groqBaseUrl.trim() } : {}),
      ...(form.provider === 'gemini' ? {
        google_cloud_project: form.googleCloudProject.trim(),
        google_cloud_location: form.googleCloudLocation.trim(),
      } : {}),
      ...(form.provider === 'bedrock' ? {
        bedrock_region: form.bedrockRegion.trim(),
        bedrock_profile: form.bedrockProfile.trim(),
      } : {}),
    });
  };

  const attachCredentials = async () => {
    setCredentialBusy(true);
    try {
      await onAttachGeminiCredentials();
    } finally {
      setCredentialBusy(false);
    }
  };

  const clearCredentials = async () => {
    setCredentialBusy(true);
    try {
      await onClearGeminiCredentials();
    } finally {
      setCredentialBusy(false);
    }
  };

  return (
    <section className="settings-panel glass" aria-labelledby="settings-title">
      <div className="settings-heading">
        <div>
          <span className="section-kicker">Production settings</span>
          <h2 id="settings-title">Copilot provider</h2>
        </div>
        <button type="button" className="close-button" onClick={onClose} aria-label="Close settings">×</button>
      </div>
      {loading ? <div className="loading-state"><span className="loader" />Loading local settings…</div> : (
        <form className="settings-form" onSubmit={submit}>
          <p className="settings-intro">Provider credentials are stored only in your private Intent configuration and are never shown again.</p>
          <label className="settings-field">
            <span>Provider</span>
            <select value={form.provider} onChange={(event) => update('provider', event.target.value as LLMProvider)} disabled={saving || credentialBusy}>
              {PROVIDERS.map((item) => <option key={item.value} value={item.value}>{item.label} — {item.description}</option>)}
            </select>
          </label>
          {form.provider === 'gemini' ? (
            <div className="settings-field">
              <span>Service account JSON</span>
              <p className="settings-intro">
                {credentialsConfigured
                  ? 'A Gemini service account JSON is saved locally.'
                  : 'Attach a Google Cloud service account JSON from this app. API keys are not supported for Gemini.'}
              </p>
              {electronAvailable ? (
                <div className="settings-actions">
                  <button className="compact-primary" type="button" onClick={() => void attachCredentials()} disabled={saving || credentialBusy}>
                    {credentialBusy ? 'Working…' : credentialsConfigured ? 'Replace JSON file' : 'Attach JSON file'}
                  </button>
                  {credentialsConfigured && (
                    <button className="compact-quiet" type="button" onClick={() => void clearCredentials()} disabled={saving || credentialBusy}>
                      Remove credentials
                    </button>
                  )}
                </div>
              ) : (
                <p className="settings-error">Gemini credentials can only be attached from the installed Intent desktop app.</p>
              )}
            </div>
          ) : (
            <>
              <label className="settings-field">
                <span>{keyLabel}</span>
                <input type="password" value={form.apiKey} onChange={(event) => update('apiKey', event.target.value)} placeholder={keyConfigured ? 'Saved locally — leave blank to keep it' : 'Paste your API key'} autoComplete="new-password" spellCheck="false" disabled={saving || credentialBusy} />
              </label>
              {keyConfigured && <button className="text-button settings-remove-key" type="button" onClick={removeSavedKey} disabled={saving || credentialBusy}>Remove saved {provider.label} key</button>}
            </>
          )}
          <label className="settings-field">
            <span>Model override <em>optional</em></span>
            <input value={form.model} onChange={(event) => update('model', event.target.value)} placeholder="Use the provider default" autoComplete="off" spellCheck="false" disabled={saving || credentialBusy} />
          </label>
          {form.provider === 'groq' && (
            <label className="settings-field">
              <span>Base URL</span>
              <input value={form.groqBaseUrl} onChange={(event) => update('groqBaseUrl', event.target.value)} autoComplete="off" spellCheck="false" disabled={saving || credentialBusy} />
            </label>
          )}
          {form.provider === 'gemini' && (
            <div className="settings-field-row">
              <label className="settings-field"><span>Google Cloud project <em>optional</em></span><input value={form.googleCloudProject} onChange={(event) => update('googleCloudProject', event.target.value)} autoComplete="off" spellCheck="false" disabled={saving || credentialBusy} /></label>
              <label className="settings-field"><span>Location</span><input value={form.googleCloudLocation} onChange={(event) => update('googleCloudLocation', event.target.value)} autoComplete="off" spellCheck="false" disabled={saving || credentialBusy} /></label>
            </div>
          )}
          {form.provider === 'bedrock' && (
            <div className="settings-field-row">
              <label className="settings-field"><span>AWS region</span><input value={form.bedrockRegion} onChange={(event) => update('bedrockRegion', event.target.value)} placeholder="us-east-1" autoComplete="off" spellCheck="false" disabled={saving || credentialBusy} /></label>
              <label className="settings-field"><span>AWS profile <em>optional</em></span><input value={form.bedrockProfile} onChange={(event) => update('bedrockProfile', event.target.value)} autoComplete="off" spellCheck="false" disabled={saving || credentialBusy} /></label>
            </div>
          )}
          <label className="settings-toggle">
            <input type="checkbox" checked={form.enabled} onChange={(event) => update('enabled', event.target.checked)} disabled={saving || credentialBusy} />
            <span><strong>Enable Copilot</strong><small>Use this provider only when you ask a Copilot question.</small></span>
          </label>
          {error && <p className="settings-error" role="alert">{error}</p>}
          <div className="settings-actions">
            <button className="compact-quiet" type="button" onClick={onClose} disabled={saving || credentialBusy}>Cancel</button>
            <button className="compact-primary" type="submit" disabled={saving || credentialBusy}>{saving ? 'Saving…' : 'Save provider'}</button>
          </div>
        </form>
      )}
    </section>
  );
}
