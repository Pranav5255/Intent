//! Detailed-capture consent.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use intent_types::{Payload, is_browser_source};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{ConfigError, JsonConfig};
use crate::paths::resolve_user_path;
use crate::secrets::is_sensitive_path;

/// Event kinds that need explicit opt-in.
pub const DETAILED_EVENT_KINDS: &[(&str, &str)] = &[
    ("vscode", "document_change"),
    ("firefox", "user_action"),
    ("chrome", "user_action"),
    ("filesystem", "file_content"),
];

pub const DEFAULT_EXCLUDED_PATTERNS: &[&str] =
    &[".env", ".env.*", "*.pem", "*.key", "id_rsa*", "*secret*", "*credential*", "*password*", "*token*"];

pub fn is_detailed_event(source: &str, kind: &str) -> bool {
    DETAILED_EVENT_KINDS.contains(&(source, kind))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetailedKind {
    Editor,
    Browser,
    Filesystem,
}

/// Compiled, case-insensitive `fnmatch`-style basename patterns.
#[derive(Debug, Default)]
struct PatternMatcher {
    globs: Option<GlobSet>,
    /// Patterns globset cannot compile; compared literally (fnmatch treats bad
    /// syntax such as a lone `[` as a literal).
    literals: Vec<String>,
}

impl PatternMatcher {
    fn new(patterns: &[String]) -> Self {
        let mut builder = GlobSetBuilder::new();
        let mut literals = Vec::new();
        let mut any = false;
        for pattern in patterns {
            let lowered = pattern.to_lowercase();
            match GlobBuilder::new(&lowered)
                .case_insensitive(true)
                .literal_separator(false)
                .backslash_escape(false)
                .build()
            {
                Ok(glob) => {
                    builder.add(glob);
                    any = true;
                }
                Err(_) => literals.push(lowered),
            }
        }
        let globs = if any { builder.build().ok() } else { None };
        PatternMatcher { globs, literals }
    }

    fn matches(&self, basename: &str) -> bool {
        let lowered = basename.to_lowercase();
        self.globs.as_ref().is_some_and(|set| set.is_match(&lowered)) || self.literals.contains(&lowered)
    }
}

#[derive(Debug, Clone, Default)]
struct LazyMatcher(OnceLock<Arc<PatternMatcher>>);

impl PartialEq for LazyMatcher {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorConfig {
    pub enabled: bool,
    excluded_patterns: Vec<String>,
    #[serde(skip)]
    matcher: LazyMatcher,
}

impl EditorConfig {
    pub fn new(enabled: bool, excluded_patterns: Vec<String>) -> Self {
        EditorConfig { enabled, excluded_patterns, matcher: LazyMatcher::default() }
    }

    pub fn excluded_patterns(&self) -> &[String] {
        &self.excluded_patterns
    }

    pub fn set_excluded_patterns(&mut self, patterns: Vec<String>) {
        self.excluded_patterns = patterns;
        self.matcher = LazyMatcher::default();
    }

    /// Whether a basename matches a user/default exclusion pattern.
    pub fn is_excluded_basename(&self, basename: &str) -> bool {
        self.matcher.0.get_or_init(|| Arc::new(PatternMatcher::new(&self.excluded_patterns))).matches(basename)
    }
}

impl Default for EditorConfig {
    fn default() -> Self {
        EditorConfig::new(false, DEFAULT_EXCLUDED_PATTERNS.iter().map(|p| p.to_string()).collect())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserConfig {
    pub enabled: bool,
    pub context_enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesystemDetailConfig {
    pub enabled: bool,
}

/// `detailed-capture.json`. Everything is off by default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DetailedCaptureConfig {
    pub editor: EditorConfig,
    pub browser: BrowserConfig,
    pub filesystem: FilesystemDetailConfig,
}

impl DetailedCaptureConfig {
    pub fn set_enabled(&mut self, kind: DetailedKind, enabled: bool) {
        match kind {
            DetailedKind::Editor => self.editor.enabled = enabled,
            DetailedKind::Browser => self.browser.enabled = enabled,
            DetailedKind::Filesystem => self.filesystem.enabled = enabled,
        }
    }

    /// Consent for a `(source, type)`; non-detailed kinds are always allowed.
    pub fn is_enabled(&self, source: &str, kind: &str) -> bool {
        match (source, kind) {
            ("vscode", "document_change") => self.editor.enabled,
            (s, "user_action") if is_browser_source(s) => self.browser.enabled,
            ("filesystem", "file_content") => self.filesystem.enabled,
            _ => true,
        }
    }

    /// Server-side editor gate: the file must be inside its (resolved) workspace, the
    /// workspace must be approved, and the path must not be sensitive or excluded.
    ///
    /// `approved_workspaces` are expected to be resolved already (as
    /// [`super::WorkspacesConfig`] stores them).
    pub fn editor_event_is_approved(&self, payload: &Payload, approved_workspaces: &[String]) -> bool {
        let (Some(Value::String(path)), Some(Value::String(workspace))) =
            (payload.get("path"), payload.get("workspace"))
        else {
            return false;
        };
        if path.is_empty() || workspace.is_empty() {
            return false;
        }
        let candidate = PathBuf::from(resolve_user_path(path));
        let workspace = PathBuf::from(resolve_user_path(workspace));
        if !candidate.starts_with(&workspace) {
            return false;
        }
        if !approved_workspaces.iter().any(|approved| Path::new(approved) == workspace) {
            return false;
        }
        if is_sensitive_path(&candidate) {
            return false;
        }
        let basename = candidate.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        !self.editor.is_excluded_basename(&basename)
    }

    /// Non-secret configuration for extensions and status (`GET /v1/detailed-capture/config`).
    pub fn public_config(&self, approved_workspaces: &[String], retention_summary: &str) -> Value {
        json!({
            "editor": {"enabled": self.editor.enabled, "excluded_patterns": self.editor.excluded_patterns},
            "browser": {"enabled": self.browser.enabled, "context_enabled": self.browser.context_enabled},
            "filesystem": {"enabled": self.filesystem.enabled},
            "approved_workspaces": approved_workspaces,
            "retention": retention_summary,
            "export_includes_details": true,
        })
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.editor.excluded_patterns.iter().any(String::is_empty) {
            return Err(ConfigError::invalid("detailed-capture editor.excluded_patterns must be a list of strings"));
        }
        Ok(())
    }
}

impl JsonConfig for DetailedCaptureConfig {
    fn default_config() -> Self {
        DetailedCaptureConfig::default()
    }

    fn from_json(raw: &Value) -> Result<Self, ConfigError> {
        let Value::Object(raw) = raw else {
            return Err(ConfigError::invalid("detailed-capture config must be an object"));
        };
        let empty = serde_json::Map::new();
        let mut config = DetailedCaptureConfig::default();
        for (source, kind) in [
            ("editor", DetailedKind::Editor),
            ("browser", DetailedKind::Browser),
            ("filesystem", DetailedKind::Filesystem),
        ] {
            let section = match raw.get(source) {
                None => &empty,
                Some(Value::Object(section)) => section,
                Some(_) => {
                    return Err(ConfigError::invalid(format!("detailed-capture {source} config must be an object")));
                }
            };
            match section.get("enabled") {
                None => {}
                Some(Value::Bool(enabled)) => config.set_enabled(kind, *enabled),
                Some(_) => {
                    return Err(ConfigError::invalid(format!("detailed-capture {source}.enabled must be a boolean")));
                }
            }
        }
        let section = |name: &str| raw.get(name).and_then(Value::as_object);
        match section("browser").and_then(|browser| browser.get("context_enabled")) {
            None => {}
            Some(Value::Bool(enabled)) => config.browser.context_enabled = *enabled,
            Some(_) => {
                return Err(ConfigError::invalid("detailed-capture browser.context_enabled must be a boolean"));
            }
        }
        if let Some(patterns) = section("editor").and_then(|editor| editor.get("excluded_patterns")) {
            let invalid =
                || ConfigError::invalid("detailed-capture editor.excluded_patterns must be a list of strings");
            let Value::Array(items) = patterns else {
                return Err(invalid());
            };
            let patterns = items
                .iter()
                .map(|item| item.as_str().filter(|s| !s.is_empty()).map(str::to_owned).ok_or_else(invalid))
                .collect::<Result<Vec<_>, _>>()?;
            config.editor.set_excluded_patterns(patterns);
        }
        config.validate()?;
        Ok(config)
    }

    fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    fn decode_error(path: &Path) -> String {
        format!("invalid detailed-capture config: {}", path.display())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetailedCaptureEditorUpdate {
    pub enabled: Option<bool>,
    pub excluded_patterns: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetailedCaptureBrowserUpdate {
    pub enabled: Option<bool>,
    pub context_enabled: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetailedCaptureFilesystemUpdate {
    pub enabled: Option<bool>,
}

/// `PUT /v1/detailed-capture/config` body: partial update.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetailedCaptureUpdate {
    pub editor: Option<DetailedCaptureEditorUpdate>,
    pub browser: Option<DetailedCaptureBrowserUpdate>,
    pub filesystem: Option<DetailedCaptureFilesystemUpdate>,
}

impl DetailedCaptureUpdate {
    pub fn apply(&self, config: &mut DetailedCaptureConfig) {
        if let Some(editor) = &self.editor {
            if let Some(enabled) = editor.enabled {
                config.editor.enabled = enabled;
            }
            if let Some(patterns) = &editor.excluded_patterns {
                config.editor.set_excluded_patterns(patterns.clone());
            }
        }
        if let Some(browser) = &self.browser {
            if let Some(enabled) = browser.enabled {
                config.browser.enabled = enabled;
            }
            if let Some(context) = browser.context_enabled {
                config.browser.context_enabled = context;
            }
        }
        if let Some(enabled) = self.filesystem.as_ref().and_then(|fs| fs.enabled) {
            config.filesystem.enabled = enabled;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: Value) -> Result<DetailedCaptureConfig, ConfigError> {
        DetailedCaptureConfig::from_json(&value)
    }

    #[test]
    fn defaults_are_off_with_default_patterns() {
        let config = DetailedCaptureConfig::default();
        assert!(
            !config.editor.enabled
                && !config.browser.enabled
                && !config.browser.context_enabled
                && !config.filesystem.enabled
        );
        assert_eq!(config.editor.excluded_patterns(), DEFAULT_EXCLUDED_PATTERNS);
        assert_eq!(parse(json!({})).unwrap(), config);
        assert_eq!(
            config.to_json(),
            json!({"editor": {"enabled": false, "excluded_patterns": DEFAULT_EXCLUDED_PATTERNS},
                   "browser": {"enabled": false, "context_enabled": false},
                   "filesystem": {"enabled": false}})
        );
    }

    #[test]
    fn normalisation_and_errors() {
        let config = parse(json!({"editor": {"enabled": true, "excluded_patterns": ["*.log"]}, "browser": {"context_enabled": true}, "extra": 1})).unwrap();
        assert!(config.editor.enabled);
        assert_eq!(config.editor.excluded_patterns(), ["*.log"]);
        assert!(config.browser.context_enabled && !config.browser.enabled);
        let err = |value: Value| parse(value).unwrap_err().to_string();
        assert_eq!(err(json!([])), "detailed-capture config must be an object");
        assert_eq!(err(json!({"browser": 1})), "detailed-capture browser config must be an object");
        assert_eq!(
            err(json!({"filesystem": {"enabled": "yes"}})),
            "detailed-capture filesystem.enabled must be a boolean"
        );
        assert_eq!(err(json!({"editor": {"enabled": null}})), "detailed-capture editor.enabled must be a boolean");
        assert_eq!(
            err(json!({"browser": {"context_enabled": 1}})),
            "detailed-capture browser.context_enabled must be a boolean"
        );
        for bad in [json!("*.log"), json!([""]), json!([1]), json!(null)] {
            assert_eq!(
                err(json!({"editor": {"excluded_patterns": bad}})),
                "detailed-capture editor.excluded_patterns must be a list of strings"
            );
        }
        assert!(parse(json!({"editor": {"excluded_patterns": []}})).unwrap().editor.excluded_patterns().is_empty());
    }

    #[test]
    fn detailed_kinds_and_consent() {
        assert!(is_detailed_event("vscode", "document_change"));
        assert!(is_detailed_event("firefox", "user_action"));
        assert!(is_detailed_event("filesystem", "file_content"));
        assert!(!is_detailed_event("firefox", "tab_change"));
        let mut config = DetailedCaptureConfig::default();
        assert!(!config.is_enabled("vscode", "document_change"));
        assert!(config.is_enabled("shell", "command"));
        config.set_enabled(DetailedKind::Browser, true);
        assert!(config.is_enabled("firefox", "user_action"));
        assert!(!config.is_enabled("filesystem", "file_content"));
        config.set_enabled(DetailedKind::Filesystem, true);
        assert!(config.is_enabled("filesystem", "file_content"));
    }

    #[test]
    fn update_applies_partially() {
        let mut config = DetailedCaptureConfig::default();
        let update: DetailedCaptureUpdate = serde_json::from_value(
            json!({"editor": {"excluded_patterns": ["*.tmp"]}, "browser": {"context_enabled": true}}),
        )
        .unwrap();
        update.apply(&mut config);
        assert!(!config.editor.enabled);
        assert_eq!(config.editor.excluded_patterns(), ["*.tmp"]);
        assert!(config.browser.context_enabled);
        assert!(config.editor.is_excluded_basename("x.TMP"));
        DetailedCaptureUpdate {
            filesystem: Some(DetailedCaptureFilesystemUpdate { enabled: Some(true) }),
            ..Default::default()
        }
        .apply(&mut config);
        assert!(config.filesystem.enabled);
    }

    #[test]
    fn fnmatch_semantics() {
        let mut config = DetailedCaptureConfig::default();
        config.editor.set_excluded_patterns(vec![
            "*.LOG".into(),
            "data?.csv".into(),
            "[ab]*.txt".into(),
            "[".into(),
            "build".into(),
        ]);
        let editor = &config.editor;
        assert!(editor.is_excluded_basename("app.log"));
        assert!(editor.is_excluded_basename("data1.csv"));
        assert!(!editor.is_excluded_basename("data10.csv"));
        assert!(editor.is_excluded_basename("alpha.txt"));
        assert!(!editor.is_excluded_basename("zeta.txt"));
        assert!(editor.is_excluded_basename("["));
        assert!(editor.is_excluded_basename("BUILD"));
        assert!(!editor.is_excluded_basename("builder"));
    }

    fn approved_case() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::paths::resolve_lenient(dir.path()).to_string_lossy().into_owned();
        (dir, root)
    }

    fn payload(path: &str, workspace: &str) -> Payload {
        json!({"path": path, "workspace": workspace, "changes": []}).as_object().cloned().unwrap()
    }

    #[test]
    fn editor_approval_requires_approved_workspace() {
        let (_dir, root) = approved_case();
        let mut config = DetailedCaptureConfig::default();
        config.editor.enabled = true;
        let file = format!("{root}/src/main.rs");
        assert!(config.editor_event_is_approved(&payload(&file, &root), std::slice::from_ref(&root)));
        assert!(!config.editor_event_is_approved(&payload(&file, &root), &[]));
        assert!(!config.editor_event_is_approved(&payload(&file, &root), &[format!("{root}/src")]));
        // Normalised workspace spelling still matches.
        assert!(
            config.editor_event_is_approved(&payload(&file, &format!("{root}/src/..")), std::slice::from_ref(&root))
        );
    }

    #[test]
    fn editor_approval_rejects_escape_and_bad_payloads() {
        let (_dir, root) = approved_case();
        let config = DetailedCaptureConfig::default();
        let approved = std::slice::from_ref(&root);
        assert!(!config.editor_event_is_approved(&payload(&format!("{root}/../outside.rs"), &root), approved));
        assert!(!config.editor_event_is_approved(&payload("/etc/hosts", &root), approved));
        assert!(!config.editor_event_is_approved(&payload(&format!("{root}x/a.rs"), &root), approved));
        assert!(
            !config.editor_event_is_approved(
                &json!({"path": 1, "workspace": root}).as_object().cloned().unwrap(),
                approved
            )
        );
        assert!(
            !config.editor_event_is_approved(
                &json!({"path": format!("{root}/a")}).as_object().cloned().unwrap(),
                approved
            )
        );
        assert!(!config.editor_event_is_approved(&payload("", &root), approved));
    }

    #[test]
    fn editor_approval_rejects_sensitive_and_excluded() {
        let (_dir, root) = approved_case();
        let mut config = DetailedCaptureConfig::default();
        let approved = std::slice::from_ref(&root);
        for name in [
            ".env",
            ".env.production",
            "server.pem",
            "tls.key",
            "id_rsa",
            "my_secrets.rs",
            "Credentials.md",
            "token_store.py",
            ".ssh/config",
        ] {
            assert!(!config.editor_event_is_approved(&payload(&format!("{root}/{name}"), &root), approved), "{name}");
        }
        config.editor.set_excluded_patterns(vec!["*.lock".into()]);
        assert!(!config.editor_event_is_approved(&payload(&format!("{root}/Cargo.lock"), &root), approved));
        // Built-in sensitive rules apply even when custom patterns replace the defaults.
        assert!(!config.editor_event_is_approved(&payload(&format!("{root}/.env"), &root), approved));
        assert!(config.editor_event_is_approved(&payload(&format!("{root}/Cargo.toml"), &root), approved));
    }

    #[test]
    fn public_config_shape() {
        let config = DetailedCaptureConfig::default();
        let value = config.public_config(&["/w".into()], "metadata:30d");
        assert_eq!(
            value,
            json!({
                "editor": {"enabled": false, "excluded_patterns": DEFAULT_EXCLUDED_PATTERNS},
                "browser": {"enabled": false, "context_enabled": false},
                "filesystem": {"enabled": false},
                "approved_workspaces": ["/w"],
                "retention": "metadata:30d",
                "export_includes_details": true,
            })
        );
    }
}
