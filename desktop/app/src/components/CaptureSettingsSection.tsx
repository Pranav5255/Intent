import { useEffect, useState, type FormEvent } from 'react';
import type { CapturePreset, DetailedCaptureSettings, DetailedCaptureSettingsUpdate } from '../types';

interface CaptureSettingsSectionProps {
  capture: DetailedCaptureSettings | null;
  filesystemAccessible: boolean;
  saving: boolean;
  error: string | null;
  onSave: (update: DetailedCaptureSettingsUpdate, filesystemAccessible?: boolean) => Promise<void>;
}

function presetFromCapture(capture: DetailedCaptureSettings | null): CapturePreset {
  if (!capture?.browser.enabled) return 'metadata';
  if (capture.browser.context_enabled) return 'actions_excerpts';
  return 'actions';
}

function updateFromPreset(preset: CapturePreset): DetailedCaptureSettingsUpdate {
  if (preset === 'metadata') {
    return { browser: { enabled: false, context_enabled: false } };
  }
  if (preset === 'actions') {
    return { browser: { enabled: true, context_enabled: false } };
  }
  return { browser: { enabled: true, context_enabled: true } };
}

const PRESETS: Array<{ id: CapturePreset; title: string; body: string }> = [
  { id: 'metadata', title: 'Metadata only', body: 'Tab URLs and titles when you switch tabs.' },
  { id: 'actions', title: 'Actions', body: 'Clicks, scroll depth, and form submits without page text.' },
  { id: 'actions_excerpts', title: 'Actions + excerpts', body: 'Short page excerpts on actions you take.' },
];

export function CaptureSettingsSection({
  capture,
  filesystemAccessible,
  saving,
  error,
  onSave,
}: CaptureSettingsSectionProps) {
  const [preset, setPreset] = useState<CapturePreset>('metadata');
  const [editorEnabled, setEditorEnabled] = useState(false);
  const [filesystemEnabled, setFilesystemEnabled] = useState(false);
  const [allAccessible, setAllAccessible] = useState(filesystemAccessible);

  useEffect(() => {
    if (!capture) return;
    setPreset(presetFromCapture(capture));
    setEditorEnabled(capture.editor.enabled);
    setFilesystemEnabled(capture.filesystem.enabled);
  }, [capture]);

  useEffect(() => {
    setAllAccessible(filesystemAccessible);
  }, [filesystemAccessible]);

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    void onSave({
      ...updateFromPreset(preset),
      editor: { enabled: editorEnabled },
      filesystem: { enabled: filesystemEnabled },
    }, allAccessible);
  };

  return (
    <form className="settings-form" onSubmit={submit}>
      <p className="settings-intro">Choose what browsing and workspace activity is stored locally on this machine.</p>
      <fieldset className="settings-field">
        <legend>Browsing capture</legend>
        {PRESETS.map((item) => (
          <label className="settings-toggle" key={item.id}>
            <input
              type="radio"
              name="capture-preset"
              checked={preset === item.id}
              onChange={() => setPreset(item.id)}
              disabled={saving}
            />
            <span><strong>{item.title}</strong><small>{item.body}</small></span>
          </label>
        ))}
      </fieldset>
      <label className="settings-toggle">
        <input type="checkbox" checked={editorEnabled} onChange={(event) => setEditorEnabled(event.target.checked)} disabled={saving} />
        <span><strong>Editor detailed capture</strong><small>Bounded VS Code change capture for approved workspaces.</small></span>
      </label>
      <label className="settings-toggle">
        <input type="checkbox" checked={filesystemEnabled} onChange={(event) => setFilesystemEnabled(event.target.checked)} disabled={saving} />
        <span><strong>Filesystem detailed capture</strong><small>Consented file content excerpts from watched paths.</small></span>
      </label>
      <label className="settings-toggle">
        <input type="checkbox" checked={allAccessible} onChange={(event) => setAllAccessible(event.target.checked)} disabled={saving} />
        <span><strong>Watch all accessible files</strong><small>Broad filesystem observation when detailed capture is enabled.</small></span>
      </label>
      {capture && capture.approved_workspaces.length > 0 && (
        <div className="settings-field">
          <span>Approved workspaces</span>
          <ul className="settings-list">
            {capture.approved_workspaces.map((path) => <li key={path}>{path}</li>)}
          </ul>
        </div>
      )}
      {error && <p className="settings-error" role="alert">{error}</p>}
      <div className="settings-actions">
        <button className="compact-primary" type="submit" disabled={saving}>{saving ? 'Saving…' : 'Save capture settings'}</button>
      </div>
    </form>
  );
}
