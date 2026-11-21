import { useEffect, useMemo, useState } from 'react';
import { Icon } from '../components/Icon';
import { StatusDot, Switch } from '../components/ui';
import { api } from '../lib/api';
import { errorText, toast, useAsync, useRetryOnReconnect } from '../lib/store';
import type { LlmProvider, LlmSettingsSummary, LlmSettingsUpdate } from '../lib/types';
import { Group, Row, Section } from './kit';

const PROVIDERS: Array<{ id: LlmProvider; label: string; where: string }> = [
  { id: 'local', label: 'Local model on this computer', where: 'this computer' },
  { id: 'openai', label: 'OpenAI', where: 'OpenAI' },
  { id: 'groq', label: 'Groq', where: 'Groq' },
  { id: 'gemini', label: 'Google Gemini', where: 'Google' },
  { id: 'bedrock', label: 'Amazon Bedrock', where: 'Amazon Web Services' },
  { id: 'none', label: 'None — rules and on-device search only', where: '' },
];

const DEFAULTS: Partial<Record<LlmProvider, { model: string; base_url?: string; embedding?: string }>> = {
  local: { model: 'qwen3:8b', base_url: 'http://127.0.0.1:11434/v1', embedding: 'nomic-embed-text' },
  openai: { model: 'gpt-5-mini', embedding: 'text-embedding-3-small' },
  groq: { model: 'llama-3.3-70b-versatile', base_url: 'https://api.groq.com/openai/v1' },
  gemini: { model: 'gemini-2.5-flash', embedding: 'text-embedding-004' },
  bedrock: { model: 'anthropic.claude-haiku-4-5', embedding: 'amazon.titan-embed-text-v2:0' },
};

interface Form {
  provider: LlmProvider;
  model: string;
  base_url: string;
  embedding_model: string;
  api_key: string;
  google_cloud_project: string;
  google_cloud_location: string;
  bedrock_region: string;
  bedrock_profile: string;
  copilot: boolean;
  cluster: boolean;
  content: boolean;
}

function formFrom(s: LlmSettingsSummary): Form {
  return {
    provider: s.provider,
    model: s.model ?? '',
    base_url: s.base_url ?? '',
    embedding_model: s.embedding_model ?? '',
    api_key: '',
    google_cloud_project: s.google_cloud_project ?? '',
    google_cloud_location: s.google_cloud_location ?? '',
    bedrock_region: s.bedrock_region ?? '',
    bedrock_profile: s.bedrock_profile ?? '',
    copilot: s.copilot_enabled,
    cluster: s.semantic_cluster_enabled,
    content: s.semantic_content_consent,
  };
}

function isLoopback(url: string): boolean {
  try {
    const host = new URL(url).hostname;
    return host === '127.0.0.1' || host === 'localhost' || host === '::1' || host === '[::1]';
  } catch {
    return false;
  }
}

export function IntelligenceSection() {
  const settings = useAsync(() => api.llmSettings(), []);
  useRetryOnReconnect(settings);
  const [form, setForm] = useState<Form | null>(null);
  const [saving, setSaving] = useState(false);
  const [tested, setTested] = useState<boolean | null>(null);

  useEffect(() => {
    if (settings.data) {
      setForm(formFrom(settings.data));
      setTested(settings.data.local_server_reachable);
    }
  }, [settings.data]);

  const dirty = useMemo(() => {
    if (!form || !settings.data) return false;
    const base = formFrom(settings.data);
    return (Object.keys(form) as Array<keyof Form>).some((k) => form[k] !== base[k]);
  }, [form, settings.data]);

  if (!form) {
    return (
      <Section id="intelligence" title="Intelligence" icon="chip">
        <Group>
          <Row title="Model" subtitle={settings.error ? settings.error.message : 'Loading…'} keywords="provider model llm" />
        </Group>
      </Section>
    );
  }

  const set = <K extends keyof Form>(k: K, v: Form[K]) => setForm((f) => (f ? { ...f, [k]: v } : f));
  const p = form.provider;
  const cloud = p !== 'local' && p !== 'none';
  const providerInfo = PROVIDERS.find((x) => x.id === p)!;

  const save = async (): Promise<LlmSettingsSummary | null> => {
    setSaving(true);
    const update: LlmSettingsUpdate = {
      provider: p,
      model: form.model || null,
      base_url: p === 'local' || p === 'groq' || p === 'openai' ? form.base_url || null : null,
      embedding_model: form.embedding_model || null,
      api_key: cloud && form.api_key ? form.api_key : null,
      enable_copilot: form.copilot,
      enable_semantic_cluster: form.cluster,
      enable_semantic_content_consent: form.content,
      google_cloud_project: p === 'gemini' ? form.google_cloud_project || null : null,
      google_cloud_location: p === 'gemini' ? form.google_cloud_location || null : null,
      bedrock_region: p === 'bedrock' ? form.bedrock_region || null : null,
      bedrock_profile: p === 'bedrock' ? form.bedrock_profile || null : null,
    };
    try {
      const saved = await api.saveLlmSettings(update);
      settings.setData(() => saved);
      return saved;
    } catch (r) {
      toast({ tone: 'error', text: 'Couldn’t save model settings', detail: errorText(r) });
      return null;
    } finally {
      setSaving(false);
    }
  };

  const changeProvider = (next: LlmProvider) => {
    const d = DEFAULTS[next];
    setForm((f) =>
      f
        ? {
            ...f,
            provider: next,
            model: next === settings.data?.provider ? (settings.data.model ?? '') : (d?.model ?? ''),
            base_url: next === settings.data?.provider ? (settings.data.base_url ?? '') : (d?.base_url ?? ''),
            embedding_model: next === settings.data?.provider ? (settings.data.embedding_model ?? '') : (d?.embedding ?? ''),
            api_key: '',
          }
        : f,
    );
    setTested(null);
  };

  const keyConfigured = settings.data?.provider === p && settings.data.api_key_configured;

  return (
    <Section id="intelligence" title="Intelligence" icon="chip">
      <Group
        title="Model"
        description="Intent uses a language model to name threads, group related work and answer questions. A local model keeps all of it on this computer."
      >
        <Row title="Provider" keywords="provider model llm local ollama openai groq gemini bedrock cloud ai">
          <select className="field field-md" aria-label="Provider" value={p} onChange={(e) => changeProvider(e.target.value as LlmProvider)}>
            {PROVIDERS.map((x) => (
              <option key={x.id} value={x.id}>
                {x.label}
              </option>
            ))}
          </select>
        </Row>

        {(p === 'local' || p === 'groq' || p === 'openai') && (
          <Row
            title="Server address"
            subtitle={p === 'local' ? 'Ollama, llama.cpp, LM Studio or any OpenAI-compatible server' : 'Leave as is unless you use a proxy'}
            keywords="base url server address endpoint ollama llama.cpp lm studio"
          >
            <input
              className="field field-md mono"
              value={form.base_url}
              placeholder={DEFAULTS[p]?.base_url ?? 'https://api.openai.com/v1'}
              onChange={(e) => set('base_url', e.target.value)}
              aria-label="Server address"
              spellCheck={false}
            />
          </Row>
        )}

        {cloud && (
          <Row
            title="API key"
            subtitle={keyConfigured ? 'A key is saved on this computer. Type a new one to replace it.' : `Stored only on this computer, readable by you alone`}
            keywords="api key secret token credentials"
          >
            <input
              className="field field-md mono"
              type="password"
              value={form.api_key}
              placeholder={keyConfigured ? '•••••••••••• saved' : 'Paste key'}
              onChange={(e) => set('api_key', e.target.value)}
              aria-label="API key"
              autoComplete="off"
            />
          </Row>
        )}

        {p === 'gemini' && (
          <>
            <Row title="Google Cloud project" subtitle="Needed for Vertex AI; leave empty with an API key" keywords="gemini google cloud project vertex">
              <input
                className="field field-md mono"
                value={form.google_cloud_project}
                placeholder="my-project-id"
                onChange={(e) => set('google_cloud_project', e.target.value)}
                aria-label="Google Cloud project"
              />
            </Row>
            <Row title="Location" keywords="gemini google cloud location region vertex">
              <input
                className="field field-md mono"
                value={form.google_cloud_location}
                placeholder="us-central1"
                onChange={(e) => set('google_cloud_location', e.target.value)}
                aria-label="Google Cloud location"
              />
            </Row>
          </>
        )}

        {p === 'bedrock' && (
          <>
            <Row title="AWS region" keywords="bedrock aws region">
              <input
                className="field field-md mono"
                value={form.bedrock_region}
                placeholder="us-east-1"
                onChange={(e) => set('bedrock_region', e.target.value)}
                aria-label="AWS region"
              />
            </Row>
            <Row title="AWS profile" subtitle="From ~/.aws/config; leave empty for the default" keywords="bedrock aws profile credentials">
              <input
                className="field field-md mono"
                value={form.bedrock_profile}
                placeholder="default"
                onChange={(e) => set('bedrock_profile', e.target.value)}
                aria-label="AWS profile"
              />
            </Row>
          </>
        )}

        {p !== 'none' && (
          <>
            <Row title="Model" keywords="model name chat">
              <input
                className="field field-md mono"
                value={form.model}
                placeholder={DEFAULTS[p]?.model}
                onChange={(e) => set('model', e.target.value)}
                aria-label="Model"
                spellCheck={false}
              />
            </Row>
            <Row title="Embedding model" subtitle="Used for search by meaning" keywords="embedding model vector semantic search">
              <input
                className="field field-md mono"
                value={form.embedding_model}
                placeholder={DEFAULTS[p]?.embedding ?? 'Provider default'}
                onChange={(e) => set('embedding_model', e.target.value)}
                aria-label="Embedding model"
                spellCheck={false}
              />
            </Row>
          </>
        )}

        {p === 'local' && (
          <Row
            title="Connection"
            lead={<StatusDot tone={tested === null ? 'idle' : tested ? 'capturing' : 'offline'} />}
            subtitle={
              tested === null
                ? 'Not checked yet'
                : tested
                  ? `Reachable at ${form.base_url || DEFAULTS.local?.base_url}`
                  : 'Not reachable. Is the server running?'
            }
            keywords="test connection reachable local server"
          >
            <button
              className="btn btn-sm"
              disabled={saving}
              onClick={() =>
                void save().then((s) => {
                  if (!s) return;
                  setTested(s.local_server_reachable);
                  toast(
                    s.local_server_reachable
                      ? { tone: 'success', text: 'Local model is reachable' }
                      : { tone: 'error', text: 'Couldn’t reach the local server', detail: form.base_url },
                  );
                })
              }
            >
              {saving ? 'Testing…' : 'Test connection'}
            </button>
          </Row>
        )}
      </Group>

      <Group title="Features">
        <Row title="Answer questions" subtitle="Ask about your work with ? in the palette" keywords="copilot questions answer ask">
          <Switch label="Answer questions" checked={form.copilot && p !== 'none'} disabled={p === 'none'} onChange={(v) => set('copilot', v)} />
        </Row>
        <Row title="Group by meaning" subtitle="Let the model decide which activity belongs to the same thread" keywords="semantic grouping cluster refine">
          <Switch label="Group by meaning" checked={form.cluster && p !== 'none'} disabled={p === 'none'} onChange={(v) => set('cluster', v)} />
        </Row>
        <Row
          title="Include page and edit excerpts"
          subtitle="Gives the model short excerpts, not just titles and names"
          keywords="content consent excerpts semantic privacy"
        >
          <Switch label="Include excerpts" checked={form.content && p !== 'none'} disabled={p === 'none'} onChange={(v) => set('content', v)} />
        </Row>
      </Group>

      <Disclosure form={form} where={providerInfo.where} />

      <div className={`savebar ${dirty ? 'is-dirty' : ''}`}>
        <span className="t2">{dirty ? 'Unsaved changes' : 'All changes saved'}</span>
        <button className="btn btn-quiet btn-sm" disabled={!dirty || saving} onClick={() => settings.data && setForm(formFrom(settings.data))}>
          Discard
        </button>
        <button
          className="btn btn-primary btn-sm"
          disabled={!dirty || saving}
          onClick={() => void save().then((s) => s && toast({ tone: 'success', text: 'Model settings saved' }))}
        >
          {saving ? 'Saving…' : 'Save'}
        </button>
      </div>
    </Section>
  );
}

/** Honest summary of what leaves the device, recomputed from the form. */
function Disclosure({ form, where }: { form: Form; where: string }) {
  const p = form.provider;
  let tone: 'local' | 'remote' = 'local';
  let headline: string;
  const sent: string[] = [];
  const never = ['Full file contents', 'Anything from blocked sites or ignored files', 'Passwords and form fields', 'Full commands and their arguments'];

  if (p === 'none') {
    headline = 'Nothing leaves this computer. Threads are named by rules and search runs on this device.';
  } else if (p === 'local' && (!form.base_url || isLoopback(form.base_url))) {
    headline = 'Nothing leaves this computer. Every request goes to the model server running here.';
  } else {
    tone = 'remote';
    let dest = where;
    if (p === 'local') {
      try {
        dest = new URL(form.base_url).host;
      } catch {
        dest = form.base_url;
      }
    }
    headline = `Some data is sent to ${dest}.`;
    sent.push('Project names, file names, site domains and kinds of commands for each thread');
    sent.push('How long each thread lasted and how many events it had');
    if (form.cluster) sent.push('Titles of windows and tabs, to group activity');
    if (form.content) sent.push('Short excerpts of pages and edits');
    if (form.copilot) sent.push('Your questions, and summaries of the threads that match them');
  }

  return (
    <div className={`disclosure is-${tone}`} role="note">
      <div className="disclosure-head">
        <Icon name={tone === 'local' ? 'lock' : 'external'} size={15} />
        <span className="disclosure-title">What leaves this device</span>
      </div>
      <p className="disclosure-text">{headline}</p>
      {sent.length > 0 && (
        <div className="disclosure-cols">
          <div>
            <div className="disclosure-label">Sent</div>
            <ul>
              {sent.map((s) => (
                <li key={s}>{s}</li>
              ))}
            </ul>
          </div>
          <div>
            <div className="disclosure-label">Never sent</div>
            <ul>
              {never.map((s) => (
                <li key={s}>{s}</li>
              ))}
            </ul>
          </div>
        </div>
      )}
    </div>
  );
}
