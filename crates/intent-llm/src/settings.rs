//! LLM settings: `llm.toml` (0600, atomic writes), one-time import of the 0.1 `llm.env`,
//! UI summary/update DTOs, provider construction and a local-server reachability probe.
//!
//! Nothing here reads or mutates process environment variables (0.1 exported every
//! setting into `os.environ`, leaking secrets into child processes).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use intent_types::{LlmProvider, LlmSettingsSummary, LlmSettingsUpdate};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{
    BedrockAuth, BedrockClient, BedrockConfig, ChatClient, GeminiAuth, GeminiClient, GeminiConfig, OpenAiCompatible,
    OpenAiConfig, ServiceAccountKey, shared_http,
};
use crate::copilot::ToolMode;
use crate::embed::{HashingEmbedder, OpenAiEmbedder, OpenAiEmbedderConfig};
use crate::harness::{CachePersistence, Harness, HarnessConfig};
use crate::label::LlmLabelProvider;
use crate::semantic::{LlmSemanticRefiner, MIN_LINK_CONFIDENCE, SemanticConfig};
use crate::traits::{Embedder, LabelProvider, SemanticRefiner};

pub const SETTINGS_FILE: &str = "llm.toml";
pub const LEGACY_SETTINGS_FILE: &str = "llm.env";
pub const GEMINI_CREDENTIALS_FILE: &str = "gemini-credentials.json";

pub const DEFAULT_LOCAL_BASE_URL: &str = "http://127.0.0.1:11434/v1";
pub const DEFAULT_LOCAL_MODEL: &str = "qwen2.5:3b";
pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_OPENAI_MODEL: &str = "gpt-4o-mini";
pub const DEFAULT_GROQ_BASE_URL: &str = "https://api.groq.com/openai/v1";
pub const DEFAULT_GROQ_MODEL: &str = "openai/gpt-oss-20b";
pub const DEFAULT_GEMINI_MODEL: &str = "gemini-2.5-flash";
pub const DEFAULT_GEMINI_LOCATION: &str = "us-central1";
pub const DEFAULT_BEDROCK_MODEL: &str = "amazon.nova-pro-v1:0";
pub const DEFAULT_SEMANTIC_TIMEOUT_MS: i64 = 60_000;

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("settings I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("settings file is invalid: {0}")]
    Parse(String),
    #[error("{0}")]
    Invalid(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LlmSettings {
    pub provider: LlmProvider,
    pub model: Option<String>,
    /// OpenAI-compatible base URL (including `/v1`) for local/openai/groq; API host override for gemini/bedrock.
    pub base_url: Option<String>,
    /// Embedding model on the same server; `None` uses the offline hashing embedder.
    pub embedding_model: Option<String>,
    /// OpenAI / Groq / Gemini API key / Bedrock API key. Optional for local servers.
    pub api_key: Option<String>,
    pub copilot_enabled: bool,
    pub semantic_cluster_enabled: bool,
    pub semantic_content_consent: bool,
    pub semantic_full_capture_consent: bool,
    pub semantic_timeout_ms: i64,
    pub label_timeout_ms: Option<i64>,
    pub min_link_confidence: f64,
    /// Concurrent provider requests; default 1 for local servers, 4 for cloud.
    pub concurrency: Option<usize>,
    pub copilot_tool_mode: ToolMode,
    /// Vertex AI service-account JSON (Gemini without an API key).
    pub gemini_credentials_path: Option<PathBuf>,
    pub google_cloud_project: Option<String>,
    pub google_cloud_location: Option<String>,
    pub bedrock_region: Option<String>,
    pub bedrock_profile: Option<String>,
}

impl Default for LlmSettings {
    fn default() -> Self {
        Self {
            provider: LlmProvider::Local,
            model: None,
            base_url: None,
            embedding_model: None,
            api_key: None,
            copilot_enabled: true,
            semantic_cluster_enabled: true,
            semantic_content_consent: false,
            semantic_full_capture_consent: false,
            semantic_timeout_ms: DEFAULT_SEMANTIC_TIMEOUT_MS,
            label_timeout_ms: None,
            min_link_confidence: MIN_LINK_CONFIDENCE,
            concurrency: None,
            copilot_tool_mode: ToolMode::Auto,
            gemini_credentials_path: None,
            google_cloud_project: None,
            google_cloud_location: None,
            bedrock_region: None,
            bedrock_profile: None,
        }
    }
}

fn provider_name(p: LlmProvider) -> &'static str {
    match p {
        LlmProvider::Local => "local",
        LlmProvider::Openai => "openai",
        LlmProvider::Groq => "groq",
        LlmProvider::Gemini => "gemini",
        LlmProvider::Bedrock => "bedrock",
        LlmProvider::None => "none",
    }
}

fn non_empty(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

impl LlmSettings {
    pub fn path(config_dir: &Path) -> PathBuf {
        config_dir.join(SETTINGS_FILE)
    }

    pub fn is_local(&self) -> bool {
        self.provider == LlmProvider::Local
    }

    pub fn effective_model(&self) -> String {
        if let Some(m) = non_empty(&self.model) {
            return m.to_owned();
        }
        match self.provider {
            LlmProvider::Local | LlmProvider::None => DEFAULT_LOCAL_MODEL,
            LlmProvider::Openai => DEFAULT_OPENAI_MODEL,
            LlmProvider::Groq => DEFAULT_GROQ_MODEL,
            LlmProvider::Gemini => DEFAULT_GEMINI_MODEL,
            LlmProvider::Bedrock => DEFAULT_BEDROCK_MODEL,
        }
        .to_owned()
    }

    pub fn effective_base_url(&self) -> Option<String> {
        if let Some(b) = non_empty(&self.base_url) {
            return Some(b.trim_end_matches('/').to_owned());
        }
        match self.provider {
            LlmProvider::Local => Some(DEFAULT_LOCAL_BASE_URL.into()),
            LlmProvider::Openai => Some(DEFAULT_OPENAI_BASE_URL.into()),
            LlmProvider::Groq => Some(DEFAULT_GROQ_BASE_URL.into()),
            _ => None,
        }
    }

    /// Semantic refinement runs when enabled; cloud providers additionally need content consent
    /// (local inference never leaves the machine).
    pub fn semantic_enabled(&self) -> bool {
        self.semantic_cluster_enabled
            && self.provider != LlmProvider::None
            && (self.is_local() || self.semantic_content_consent)
            && self.provider_configured()
    }

    pub fn full_capture_enabled(&self) -> bool {
        self.semantic_content_consent && self.semantic_full_capture_consent
    }

    pub fn credentials_configured(&self) -> bool {
        self.gemini_credentials_path
            .as_ref()
            .and_then(|p| fs::read(p).ok())
            .is_some_and(|raw| ServiceAccountKey::parse(&raw).is_ok())
    }

    pub fn api_key_configured(&self) -> bool {
        non_empty(&self.api_key).is_some()
    }

    pub fn provider_configured(&self) -> bool {
        match self.provider {
            LlmProvider::None => false,
            LlmProvider::Local => true,
            LlmProvider::Openai | LlmProvider::Groq => self.api_key_configured(),
            LlmProvider::Gemini => self.api_key_configured() || self.credentials_configured(),
            LlmProvider::Bedrock => {
                non_empty(&self.bedrock_region).is_some() && (self.api_key_configured() || cfg!(feature = "bedrock"))
            }
        }
    }

    // -- persistence ---------------------------------------------------------

    pub fn load(path: &Path) -> Result<Option<Self>, SettingsError> {
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map(Some).map_err(|e| SettingsError::Parse(e.message().to_owned())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), SettingsError> {
        self.validate()?;
        let body = toml::to_string_pretty(self).map_err(|e| SettingsError::Parse(e.to_string()))?;
        let text = format!("# Intent LLM settings. Managed by intentd; contains secrets, never commit.\n{body}");
        write_private_atomic(path, text.as_bytes())
    }

    /// Load `llm.toml`, importing a 0.1 `llm.env` once if no TOML exists yet.
    pub fn load_or_import(config_dir: &Path) -> Result<(Self, bool), SettingsError> {
        let path = Self::path(config_dir);
        if let Some(settings) = Self::load(&path)? {
            return Ok((settings, false));
        }
        let legacy = config_dir.join(LEGACY_SETTINGS_FILE);
        match fs::read_to_string(&legacy) {
            Ok(text) => {
                let settings = Self::import_llm_env(&text);
                settings.save(&path)?;
                tracing::info!(provider = provider_name(settings.provider), "imported 0.1 llm.env into llm.toml");
                Ok((settings, true))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((Self::default(), false)),
            Err(e) => Err(e.into()),
        }
    }

    /// Translate a 0.1 systemd `EnvironmentFile`.
    pub fn import_llm_env(text: &str) -> Self {
        let mut env = std::collections::HashMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let line = line.strip_prefix("export ").unwrap_or(line);
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim().trim_matches('"').trim_matches('\'');
                if !v.is_empty() {
                    env.insert(k.trim().to_owned(), v.to_owned());
                }
            }
        }
        let get = |k: &str| env.get(k).cloned();
        let truthy = |k: &str| env.get(k).is_some_and(|v| matches!(v.to_lowercase().as_str(), "true" | "1" | "yes"));
        let mut s = Self::default();
        let provider = get("LLM_PROVIDER").unwrap_or_else(|| "openai".into()).to_lowercase();
        s.provider = match provider.as_str() {
            "groq" => LlmProvider::Groq,
            "gemini" => LlmProvider::Gemini,
            "bedrock" => LlmProvider::Bedrock,
            "local" | "ollama" => LlmProvider::Local,
            _ => LlmProvider::Openai,
        };
        s.api_key = match s.provider {
            LlmProvider::Openai => get("OPENAI_API_KEY"),
            LlmProvider::Groq => get("GROQ_API_KEY"),
            LlmProvider::Bedrock => get("AWS_BEARER_TOKEN_BEDROCK"),
            _ => None,
        };
        if s.provider == LlmProvider::Groq {
            s.base_url = get("GROQ_BASE_URL").filter(|u| u.trim_end_matches('/') != DEFAULT_GROQ_BASE_URL);
        }
        s.model = get("INTENT_LLM_MODEL");
        s.gemini_credentials_path =
            get("GOOGLE_APPLICATION_CREDENTIALS").or_else(|| get("GEMINI_CREDENTIALS_PATH")).map(PathBuf::from);
        s.google_cloud_project = get("GOOGLE_CLOUD_PROJECT");
        s.google_cloud_location = get("GOOGLE_CLOUD_LOCATION");
        s.bedrock_region = get("BEDROCK_REGION");
        s.bedrock_profile = get("BEDROCK_AWS_PROFILE");
        s.copilot_enabled = truthy("ENABLE_COPILOT");
        s.semantic_cluster_enabled = truthy("ENGINE_SEMANTIC_CLUSTER");
        s.semantic_content_consent = truthy("ENGINE_SEMANTIC_CONTENT_CONSENT");
        s.semantic_full_capture_consent = truthy("ENGINE_SEMANTIC_FULL_CAPTURE_CONSENT");
        if let Some(ms) = get("ENGINE_SEMANTIC_TIMEOUT_MS").and_then(|v| v.parse::<i64>().ok()).filter(|v| *v > 0) {
            s.semantic_timeout_ms = ms.clamp(1_000, 120_000);
        }
        // 0.1 only used an LLM when ENGINE_LLM_ENABLED was true: keep that choice.
        if !truthy("ENGINE_LLM_ENABLED") {
            s.provider = LlmProvider::None;
        }
        s
    }

    // -- UI DTOs -----------------------------------------------------------

    pub fn summary(&self, local_server_reachable: Option<bool>) -> LlmSettingsSummary {
        LlmSettingsSummary {
            provider: self.provider,
            model: (self.provider != LlmProvider::None).then(|| self.effective_model()),
            base_url: self.effective_base_url(),
            embedding_model: self.embedding_model.clone(),
            copilot_enabled: self.copilot_enabled && self.provider_configured(),
            semantic_cluster_enabled: self.semantic_enabled(),
            semantic_content_consent: self.semantic_content_consent,
            semantic_full_capture_consent: self.full_capture_enabled(),
            semantic_timeout_ms: self.semantic_timeout_ms,
            api_key_configured: self.api_key_configured(),
            credentials_configured: self.credentials_configured(),
            google_cloud_project: self.google_cloud_project.clone(),
            google_cloud_location: self.google_cloud_location.clone(),
            bedrock_region: self.bedrock_region.clone(),
            bedrock_profile: self.bedrock_profile.clone(),
            local_server_reachable: local_server_reachable.filter(|_| self.is_local()),
        }
    }

    /// Apply a UI update. `None` fields are left unchanged; empty strings clear optional fields.
    pub fn apply_update(&mut self, update: LlmSettingsUpdate) -> Result<(), SettingsError> {
        let mut next = self.clone();
        if next.provider != update.provider {
            // A key for one provider is never sent to another.
            next.api_key = None;
            next.model = None;
            next.base_url = None;
        }
        next.provider = update.provider;
        if update.clear_api_key {
            next.api_key = None;
        } else if let Some(key) = update.api_key {
            next.api_key = Some(key.trim().to_owned()).filter(|k| !k.is_empty());
        }
        let set = |slot: &mut Option<String>, value: Option<String>| {
            if let Some(v) = value {
                *slot = Some(v.trim().to_owned()).filter(|v| !v.is_empty());
            }
        };
        set(&mut next.model, update.model);
        set(&mut next.base_url, update.base_url);
        set(&mut next.embedding_model, update.embedding_model);
        set(&mut next.google_cloud_project, update.google_cloud_project);
        set(&mut next.google_cloud_location, update.google_cloud_location);
        set(&mut next.bedrock_region, update.bedrock_region);
        set(&mut next.bedrock_profile, update.bedrock_profile);
        if let Some(v) = update.enable_copilot {
            next.copilot_enabled = v;
        }
        if let Some(v) = update.enable_semantic_cluster {
            next.semantic_cluster_enabled = v;
        }
        if let Some(v) = update.enable_semantic_content_consent {
            next.semantic_content_consent = v;
        }
        if let Some(v) = update.enable_semantic_full_capture_consent {
            next.semantic_full_capture_consent = v;
        }
        if let Some(ms) = update.semantic_timeout_ms {
            if !(1_000..=120_000).contains(&ms) {
                return Err(SettingsError::Invalid("semantic_timeout_ms must be between 1000 and 120000".into()));
            }
            next.semantic_timeout_ms = ms;
        }
        next.validate()?;
        *self = next;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), SettingsError> {
        let fields = [
            &self.model,
            &self.base_url,
            &self.embedding_model,
            &self.api_key,
            &self.google_cloud_project,
            &self.google_cloud_location,
            &self.bedrock_region,
            &self.bedrock_profile,
        ];
        if fields.iter().filter_map(|f| f.as_deref()).any(|v| v.chars().any(char::is_control)) {
            return Err(SettingsError::Invalid("settings values must be single-line text".into()));
        }
        if self.api_key.as_deref().is_some_and(|k| k.chars().any(char::is_whitespace)) {
            return Err(SettingsError::Invalid("API keys must not contain whitespace".into()));
        }
        if let Some(url) = non_empty(&self.base_url) {
            let parsed =
                reqwest::Url::parse(url).map_err(|_| SettingsError::Invalid("base_url must be a valid URL".into()))?;
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err(SettingsError::Invalid("base_url must use http or https".into()));
            }
            // Plain http is only acceptable for loopback servers.
            let host = parsed.host_str().unwrap_or_default();
            let loopback = matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]");
            if parsed.scheme() == "http" && !loopback && !self.is_local() {
                return Err(SettingsError::Invalid("cloud base_url must use https".into()));
            }
        }
        if !(0.0..=1.0).contains(&self.min_link_confidence) {
            return Err(SettingsError::Invalid("min_link_confidence must be between 0 and 1".into()));
        }
        Ok(())
    }
}

/// Write `bytes` to `path` atomically with mode 0600 (directory 0700).
pub fn write_private_atomic(path: &Path, bytes: &[u8]) -> Result<(), SettingsError> {
    let dir = path.parent().ok_or_else(|| SettingsError::Invalid("settings path has no parent".into()))?;
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    let tmp = dir.join(format!(
        ".{}.{}.{:x}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("settings"),
        std::process::id(),
        fastrand::u64(..)
    ));
    let result = (|| -> Result<(), SettingsError> {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Validate and store a Vertex service-account key next to the settings (0600).
/// Returns the path; callers then set `gemini_credentials_path` (and project if unset).
pub fn save_gemini_credentials(config_dir: &Path, raw: &[u8]) -> Result<(PathBuf, ServiceAccountKey), SettingsError> {
    let key = ServiceAccountKey::parse(raw).map_err(SettingsError::Invalid)?;
    let path = config_dir.join(GEMINI_CREDENTIALS_FILE);
    write_private_atomic(&path, raw)?;
    Ok((path, key))
}

// ---------------------------------------------------------------------------
// Provider construction
// ---------------------------------------------------------------------------

/// Everything the daemon needs, built from settings.
pub struct Providers {
    /// LLM labeler; `None` → the engine uses its template labels.
    pub label: Option<Arc<dyn LabelProvider>>,
    /// Semantic refiner; `None` → deterministic clusters only.
    pub semantic: Option<Arc<dyn SemanticRefiner>>,
    /// Always present: the configured embedding model or the offline hashing embedder.
    pub embedder: Arc<dyn Embedder>,
    /// Harness for the copilot (`Copilot::new(harness, backend, config)`), when enabled.
    pub copilot: Option<Arc<Harness>>,
    /// The shared harness (one limiter and cache across label/semantic/copilot).
    pub harness: Option<Arc<Harness>>,
    /// Why no LLM is available, for status display.
    pub unavailable_reason: Option<String>,
}

/// Build the chat transport for the configured provider.
pub fn build_chat_client(settings: &LlmSettings) -> Result<Option<Arc<dyn ChatClient>>, String> {
    let model = settings.effective_model();
    let timeout = Duration::from_millis(settings.semantic_timeout_ms.clamp(1_000, 600_000) as u64);
    let api_key = non_empty(&settings.api_key).map(str::to_owned);
    let openai = |provider: &str, is_local: bool| -> Arc<dyn ChatClient> {
        Arc::new(OpenAiCompatible::new(OpenAiConfig {
            provider: provider.into(),
            base_url: settings.effective_base_url().unwrap_or_default(),
            api_key: api_key.clone(),
            model: model.clone(),
            is_local,
            timeout: if is_local { timeout.max(Duration::from_secs(120)) } else { timeout },
            reasoning_effort: (provider == "groq" && model.contains("gpt-oss")).then(|| "low".into()),
        }))
    };
    Ok(Some(match settings.provider {
        LlmProvider::None => return Ok(None),
        LlmProvider::Local => openai("local", true),
        LlmProvider::Openai | LlmProvider::Groq => {
            if api_key.is_none() {
                return Err(format!("{} needs an API key", provider_name(settings.provider)));
            }
            openai(provider_name(settings.provider), false)
        }
        LlmProvider::Gemini => {
            let auth = if let Some(key) = api_key.clone() {
                GeminiAuth::ApiKey(key)
            } else {
                let path = settings
                    .gemini_credentials_path
                    .as_ref()
                    .ok_or("Gemini needs an API key or a service-account JSON")?;
                let raw = fs::read(path).map_err(|_| "Gemini service-account file is missing".to_owned())?;
                let key = ServiceAccountKey::parse(&raw)?;
                GeminiAuth::ServiceAccount {
                    key,
                    project: non_empty(&settings.google_cloud_project).map(str::to_owned),
                    location: non_empty(&settings.google_cloud_location).unwrap_or(DEFAULT_GEMINI_LOCATION).to_owned(),
                }
            };
            Arc::new(GeminiClient::new(GeminiConfig {
                auth,
                model,
                timeout,
                base_url: non_empty(&settings.base_url).map(str::to_owned),
            }))
        }
        LlmProvider::Bedrock => {
            let region = non_empty(&settings.bedrock_region).ok_or("Bedrock needs a region")?.to_owned();
            let auth = match api_key.clone() {
                Some(k) => BedrockAuth::ApiKey(k),
                None if cfg!(feature = "bedrock") => {
                    BedrockAuth::AwsChain { profile: non_empty(&settings.bedrock_profile).map(str::to_owned) }
                }
                None => return Err("Bedrock needs an API key (or a build with the `bedrock` feature)".into()),
            };
            Arc::new(BedrockClient::new(BedrockConfig {
                region,
                auth,
                model,
                timeout,
                max_tokens: 2048,
                structured_output: None,
                base_url: non_empty(&settings.base_url).map(str::to_owned),
            }))
        }
    }))
}

/// `embedding_model` values: unset → offline hashing; `model2vec:/path/to/potion-base-8M`
/// → Model2Vec (feature `model2vec`); anything else → `/v1/embeddings` model name.
fn build_embedder(settings: &LlmSettings) -> Arc<dyn Embedder> {
    let Some(model) = non_empty(&settings.embedding_model) else { return Arc::new(HashingEmbedder::default()) };
    if let Some(dir) = model.strip_prefix("model2vec:") {
        #[cfg(feature = "model2vec")]
        match crate::embed::Model2VecEmbedder::load(Path::new(dir)) {
            Ok(e) => return Arc::new(e),
            Err(e) => tracing::warn!(error = %e, "Model2Vec unavailable; using the hashing embedder"),
        }
        #[cfg(not(feature = "model2vec"))]
        tracing::warn!(dir, "built without the `model2vec` feature; using the hashing embedder");
        return Arc::new(HashingEmbedder::default());
    }
    let base_url = match settings.provider {
        LlmProvider::Local | LlmProvider::Openai | LlmProvider::Groq | LlmProvider::None => {
            settings.effective_base_url().unwrap_or_else(|| DEFAULT_LOCAL_BASE_URL.into())
        }
        // Gemini/Bedrock embeddings are not OpenAI-compatible; use the local server for embeddings.
        _ => DEFAULT_LOCAL_BASE_URL.into(),
    };
    let remote = !matches!(
        settings.provider,
        LlmProvider::Local | LlmProvider::None | LlmProvider::Gemini | LlmProvider::Bedrock
    );
    Arc::new(OpenAiEmbedder::new(OpenAiEmbedderConfig {
        provider: if remote { provider_name(settings.provider).into() } else { "local".into() },
        base_url,
        api_key: if remote { non_empty(&settings.api_key).map(str::to_owned) } else { None },
        model: model.to_owned(),
        batch_size: 32,
        timeout: Duration::from_secs(30),
    }))
}

pub fn build_providers(settings: &LlmSettings) -> Providers {
    build_providers_with(settings, None)
}

/// As [`build_providers`], with a persistence hook for the response cache.
pub fn build_providers_with(settings: &LlmSettings, persistence: Option<Arc<dyn CachePersistence>>) -> Providers {
    let embedder = build_embedder(settings);
    let client = match build_chat_client(settings) {
        Ok(Some(c)) => c,
        Ok(None) => {
            return Providers {
                label: None,
                semantic: None,
                embedder,
                copilot: None,
                harness: None,
                unavailable_reason: Some("LLM disabled".into()),
            };
        }
        Err(reason) => {
            return Providers {
                label: None,
                semantic: None,
                embedder,
                copilot: None,
                harness: None,
                unavailable_reason: Some(reason),
            };
        }
    };
    let identity = client.identity();
    let mut hconfig = HarnessConfig::for_identity(&identity);
    if let Some(c) = settings.concurrency {
        hconfig.concurrency = c.clamp(1, 32);
    }
    let harness = Arc::new(Harness::new(client, hconfig, persistence));

    let label_timeout = settings.label_timeout_ms.filter(|ms| *ms > 0).map(|ms| Duration::from_millis(ms as u64));
    let label: Arc<dyn LabelProvider> = Arc::new(LlmLabelProvider::new(Arc::clone(&harness), label_timeout));

    let semantic: Option<Arc<dyn SemanticRefiner>> = settings.semantic_enabled().then(|| {
        let mut config = SemanticConfig::for_identity(&identity);
        config.content_consent = settings.semantic_content_consent;
        config.full_capture_consent = settings.semantic_full_capture_consent;
        config.min_link_confidence = settings.min_link_confidence;
        config.timeout = Duration::from_millis(settings.semantic_timeout_ms.clamp(1_000, 600_000) as u64);
        config.json_object_mode = identity.provider == "groq" && config.full_capture();
        Arc::new(LlmSemanticRefiner::new(Arc::clone(&harness), Some(Arc::clone(&embedder)), config))
            as Arc<dyn SemanticRefiner>
    });

    Providers {
        label: Some(label),
        semantic,
        embedder,
        copilot: settings.copilot_enabled.then(|| Arc::clone(&harness)),
        harness: Some(harness),
        unavailable_reason: None,
    }
}

// ---------------------------------------------------------------------------
// Reachability probe
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LocalServerProbe {
    pub reachable: bool,
    pub models: Vec<String>,
    pub error: Option<String>,
}

/// `GET {base_url}/models` (Ollama, llama.cpp, LM Studio all serve it).
pub async fn probe_local_server(base_url: &str, timeout: Duration) -> LocalServerProbe {
    let url = format!("{}/models", base_url.trim().trim_end_matches('/'));
    match shared_http().get(&url).timeout(timeout).send().await {
        Ok(resp) if resp.status().is_success() => {
            let models = resp
                .json::<Value>()
                .await
                .ok()
                .and_then(|v| v.get("data").and_then(Value::as_array).cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_owned))
                .collect();
            LocalServerProbe { reachable: true, models, error: None }
        }
        Ok(resp) => LocalServerProbe {
            reachable: false,
            models: vec![],
            error: Some(format!("HTTP {}", resp.status().as_u16())),
        },
        Err(e) => LocalServerProbe {
            reachable: false,
            models: vec![],
            error: Some(if e.is_timeout() { "timed out".into() } else { "connection failed".into() }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_round_trip_and_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = LlmSettings::path(dir.path());
        let s = LlmSettings {
            provider: LlmProvider::Groq,
            api_key: Some("gsk_test".into()),
            semantic_content_consent: true,
            concurrency: Some(2),
            ..Default::default()
        };
        s.save(&path).unwrap();
        let back = LlmSettings::load(&path).unwrap().unwrap();
        assert_eq!(back, s);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // No temp files left behind.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn defaults_are_local_first() {
        let s = LlmSettings::default();
        assert_eq!(s.provider, LlmProvider::Local);
        assert_eq!(s.effective_base_url().as_deref(), Some(DEFAULT_LOCAL_BASE_URL));
        assert!(s.semantic_enabled(), "local semantic needs no cloud content consent");
        let summary = s.summary(Some(true));
        assert_eq!(summary.local_server_reachable, Some(true));
        assert!(!summary.api_key_configured);
    }

    #[test]
    fn imports_legacy_env() {
        let env = "# managed\nLLM_PROVIDER=groq\nENGINE_LLM_ENABLED=true\nENABLE_COPILOT=true\nGROQ_API_KEY=gsk_abc\n\
                   GROQ_BASE_URL=https://api.groq.com/openai/v1\nENGINE_SEMANTIC_CLUSTER=true\nENGINE_SEMANTIC_CONTENT_CONSENT=1\n\
                   ENGINE_SEMANTIC_TIMEOUT_MS=45000\nINTENT_LLM_MODEL=llama-3.3-70b\nOPENAI_API_KEY=sk-other\n";
        let s = LlmSettings::import_llm_env(env);
        assert_eq!(s.provider, LlmProvider::Groq);
        assert_eq!(s.api_key.as_deref(), Some("gsk_abc"));
        assert_eq!(s.base_url, None);
        assert_eq!(s.model.as_deref(), Some("llama-3.3-70b"));
        assert!(s.copilot_enabled && s.semantic_cluster_enabled && s.semantic_content_consent);
        assert!(!s.semantic_full_capture_consent);
        assert_eq!(s.semantic_timeout_ms, 45_000);

        let disabled = LlmSettings::import_llm_env("LLM_PROVIDER=openai\nOPENAI_API_KEY=sk-x\n");
        assert_eq!(disabled.provider, LlmProvider::None);
    }

    #[test]
    fn load_or_import_runs_once() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(LEGACY_SETTINGS_FILE),
            "LLM_PROVIDER=openai\nENGINE_LLM_ENABLED=true\nOPENAI_API_KEY=sk-1\n",
        )
        .unwrap();
        let (s, imported) = LlmSettings::load_or_import(dir.path()).unwrap();
        assert!(imported);
        assert_eq!(s.provider, LlmProvider::Openai);
        fs::write(dir.path().join(LEGACY_SETTINGS_FILE), "LLM_PROVIDER=groq\nENGINE_LLM_ENABLED=true\n").unwrap();
        let (s2, imported2) = LlmSettings::load_or_import(dir.path()).unwrap();
        assert!(!imported2);
        assert_eq!(s2, s);
    }

    #[test]
    fn update_semantics() {
        let mut s = LlmSettings { provider: LlmProvider::Openai, api_key: Some("sk-1".into()), ..Default::default() };
        let mut up =
            LlmSettingsUpdate { provider: LlmProvider::Openai, model: Some("gpt-x".into()), ..Default::default() };
        s.apply_update(up.clone()).unwrap();
        assert_eq!(s.api_key.as_deref(), Some("sk-1"), "None api_key keeps the stored key");
        assert_eq!(s.model.as_deref(), Some("gpt-x"));
        up.clear_api_key = true;
        s.apply_update(up).unwrap();
        assert!(s.api_key.is_none());

        let bad =
            LlmSettingsUpdate { provider: LlmProvider::Openai, semantic_timeout_ms: Some(5), ..Default::default() };
        assert!(s.apply_update(bad).is_err());
        let bad_url = LlmSettingsUpdate {
            provider: LlmProvider::Openai,
            base_url: Some("http://evil.example/v1".into()),
            ..Default::default()
        };
        assert!(s.apply_update(bad_url).is_err());

        let mut g = LlmSettings { provider: LlmProvider::Groq, api_key: Some("gsk".into()), ..Default::default() };
        g.apply_update(LlmSettingsUpdate { provider: LlmProvider::Openai, ..Default::default() }).unwrap();
        assert!(g.api_key.is_none(), "switching provider drops the old key");
    }

    #[test]
    fn build_providers_shapes() {
        let local = build_providers(&LlmSettings::default());
        assert!(local.label.is_some() && local.semantic.is_some() && local.copilot.is_some());
        assert_eq!(local.embedder.dims(), 256);
        assert_eq!(local.harness.as_ref().unwrap().config().concurrency, 1);

        let none = build_providers(&LlmSettings { provider: LlmProvider::None, ..Default::default() });
        assert!(none.label.is_none() && none.semantic.is_none());

        let missing_key = build_providers(&LlmSettings { provider: LlmProvider::Openai, ..Default::default() });
        assert!(missing_key.label.is_none());
        assert!(missing_key.unavailable_reason.unwrap().contains("API key"));

        let cloud = build_providers(&LlmSettings {
            provider: LlmProvider::Openai,
            api_key: Some("sk-x".into()),
            embedding_model: Some("text-embedding-3-small".into()),
            ..Default::default()
        });
        assert!(cloud.semantic.is_none(), "cloud semantic needs content consent");
        assert_eq!(cloud.harness.as_ref().unwrap().config().concurrency, 4);
        assert_eq!(cloud.embedder.dims(), 1536);
    }

    #[test]
    fn gemini_credentials_saved_private() {
        let dir = tempfile::tempdir().unwrap();
        let raw = br#"{"type":"service_account","project_id":"p1","private_key":"k","client_email":"e@x"}"#;
        let (path, key) = save_gemini_credentials(dir.path(), raw).unwrap();
        assert_eq!(key.project_id, "p1");
        let s =
            LlmSettings { provider: LlmProvider::Gemini, gemini_credentials_path: Some(path), ..Default::default() };
        assert!(s.credentials_configured());
        assert!(s.provider_configured());
        assert!(save_gemini_credentials(dir.path(), b"{}").is_err());
    }
}
