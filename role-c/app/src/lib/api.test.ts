import { afterEach, describe, expect, it, vi } from 'vitest';
import { roleAApi, roleBApi } from './api';
import { parseCommand } from './parseMode';

const storedPayload = {
  files: ['/workspace/taskflow/src/auth.tsx'],
  urls: ['https://developer.mozilla.org/en-US/docs/Web/HTTP/Authentication'],
  shell: { cwd: '/workspace/taskflow', last_cmd: 'npm test -- auth' },
};

afterEach(() => {
  window.history.replaceState({}, '', '/');
  window.intentOS = undefined;
});

describe('Role C backend clients', () => {
  it('asks Role B to resolve a stored resume preview before any restore', async () => {
    const request = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      body: JSON.stringify({
        needs_picker: false,
        candidates: [],
        selected: {
          intent_id: 'intent-auth',
          label: 'Build login feature',
          summary: 'JWT debugging',
          project_tag: 'taskflow',
          workspace_root: '/workspace/taskflow',
          score: 0.9,
          resume_payload: storedPayload,
        },
      }),
    });
    window.intentOS = { request, setInteractionActive: vi.fn(), setOverlayVisible: vi.fn(), onToggleOverlay: vi.fn(() => () => undefined) };

    const result = await roleBApi.selectResume({ intentId: 'intent-auth' });

    expect(result.selected?.resume_payload).toEqual(storedPayload);
    expect(request).toHaveBeenCalledWith(
      'http://127.0.0.1:9478/resume/select',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({ intent_id: 'intent-auth', restore_scope: 'same_project' }),
      }),
    );
  });

  it('posts the stored payload unchanged to Role A with only the chosen mode added', async () => {
    const request = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      body: JSON.stringify({ ok: true, restored: { files: 1, urls: 1, shell: true }, failed: [] }),
    });
    window.intentOS = { request, setInteractionActive: vi.fn(), setOverlayVisible: vi.fn(), onToggleOverlay: vi.fn(() => () => undefined) };

    await roleAApi.restore({ ...storedPayload, mode: 'continue' });

    expect(request).toHaveBeenCalledWith(
      'http://127.0.0.1:9477/v1/restore',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({ ...storedPayload, mode: 'continue' }),
      }),
    );
  });

  it('surfaces a Copilot-not-configured response without treating stored sessions as unavailable', async () => {
    const request = vi.fn().mockResolvedValue({
      ok: false,
      status: 503,
      body: JSON.stringify({ code: 'copilot_not_configured', message: 'Intent Copilot is not configured.' }),
    });
    window.intentOS = { request, setInteractionActive: vi.fn(), setOverlayVisible: vi.fn(), onToggleOverlay: vi.fn(() => () => undefined) };

    await expect(roleBApi.askCopilot('What was I working on?')).rejects.toMatchObject({
      status: 503,
      message: 'Intent Copilot is not configured.',
    });
  });

  it('sends provider settings only to the local Role B service', async () => {
    const request = vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      body: JSON.stringify({ provider: 'gemini', model: '', copilot_enabled: true, api_key_configured: true, groq_base_url: '', google_cloud_project: '', google_cloud_location: 'us-central1', bedrock_region: '', bedrock_profile: '' }),
    });
    window.intentOS = { request, setInteractionActive: vi.fn(), setOverlayVisible: vi.fn(), onToggleOverlay: vi.fn(() => () => undefined) };

    await roleBApi.saveLlmSettings({ provider: 'gemini', api_key: 'local-test-key', enable_copilot: true });

    expect(request).toHaveBeenCalledWith(
      'http://127.0.0.1:9478/settings/llm',
      expect.objectContaining({
        method: 'PUT',
        body: JSON.stringify({ provider: 'gemini', api_key: 'local-test-key', enable_copilot: true }),
      }),
    );
  });
});

describe('command routing', () => {
  it.each([
    ['/auth', { mode: 'search', query: 'auth' }],
    ['?what failed', { mode: 'copilot', query: 'what failed' }],
    ['! taskflow', { mode: 'restore', query: 'taskflow' }],
  ])('parses %s without sending it to an unintended backend endpoint', (input, expected) => {
    expect(parseCommand(input)).toEqual(expected);
  });
});
