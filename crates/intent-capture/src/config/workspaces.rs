//! Approved workspaces.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{ConfigError, JsonConfig};
use crate::paths::{Paths, resolve_lenient, resolve_user_path};

/// `workspaces.json`: resolved, de-duplicated absolute paths.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacesConfig {
    pub workspaces: Vec<String>,
}

impl WorkspacesConfig {
    /// Return a copy with `path` (resolved) appended if not present.
    pub fn with_added(&self, path: &str) -> Self {
        let resolved = resolve_user_path(path);
        let mut next = self.clone();
        if !next.workspaces.contains(&resolved) {
            next.workspaces.push(resolved);
        }
        next
    }

    /// Return a copy without `path` (resolved).
    pub fn with_removed(&self, path: &str) -> Self {
        let resolved = resolve_user_path(path);
        WorkspacesConfig { workspaces: self.workspaces.iter().filter(|item| **item != resolved).cloned().collect() }
    }

    /// Whether `path` (resolved) is an approved workspace.
    pub fn contains(&self, path: &str) -> bool {
        let resolved = resolve_user_path(path);
        self.workspaces.contains(&resolved)
    }
}

impl JsonConfig for WorkspacesConfig {
    fn default_config() -> Self {
        WorkspacesConfig::default()
    }

    fn from_json(raw: &Value) -> Result<Self, ConfigError> {
        let Value::Object(raw) = raw else {
            return Err(ConfigError::invalid("workspaces config must be an object"));
        };
        let entries = match raw.get("workspaces") {
            None => return Ok(WorkspacesConfig::default()),
            Some(Value::Array(entries)) => entries,
            Some(_) => return Err(ConfigError::invalid("workspaces must be a list")),
        };
        let mut resolved: Vec<String> = Vec::new();
        for item in entries {
            let Some(path) = item.as_str().filter(|path| !path.trim().is_empty()) else {
                return Err(ConfigError::invalid("workspace paths must be non-empty strings"));
            };
            let path = resolve_user_path(path);
            if !resolved.contains(&path) {
                resolved.push(path);
            }
        }
        Ok(WorkspacesConfig { workspaces: resolved })
    }

    fn to_json(&self) -> Value {
        json!({"workspaces": self.workspaces})
    }

    fn decode_error(path: &Path) -> String {
        format!("invalid workspaces config: {}", path.display())
    }
}

/// Folders that can never be a workspace: `/`, the home directory and its parents,
/// and any folder that holds Intent's own files. Watching those would capture far
/// more than a project, and Intent's own writes would feed back into the watcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGuard {
    home: PathBuf,
    /// Intent's config, data and runtime dirs: refused, and so is any folder in them.
    own_dirs: Vec<PathBuf>,
    /// Folders of overridden config files: only a folder that contains one is refused.
    own_files: Vec<PathBuf>,
}

impl WorkspaceGuard {
    pub fn new(paths: &Paths) -> Self {
        let mut own_dirs = vec![paths.config_dir(), paths.data_dir()];
        own_dirs.extend(paths.runtime_dir());
        let own_files = [
            paths.detailed_capture_config(),
            paths.workspaces_config(),
            paths.filesystem_capture_config(),
            paths.retention_policy_config(),
            paths.blocked_domains_config(),
            paths.token(),
            paths.database(),
        ]
        .iter()
        .filter_map(|file| file.parent().map(resolve_lenient))
        .collect();
        WorkspaceGuard {
            home: resolve_lenient(paths.home()),
            own_dirs: own_dirs.iter().map(|dir| resolve_lenient(dir)).collect(),
            own_files,
        }
    }

    /// Why the resolved `workspace` cannot be approved, or `None` if it can.
    pub fn refusal(&self, workspace: &str) -> Option<String> {
        let path = Path::new(workspace);
        if path.parent().is_none() {
            return Some("/ cannot be a workspace because it holds every file on the computer".into());
        }
        if path == self.home {
            return Some(format!(
                "{workspace} is your home directory and cannot be a workspace; add a project folder inside it"
            ));
        }
        if self.home.starts_with(path) {
            return Some(format!("{workspace} cannot be a workspace because it contains your home directory"));
        }
        let own = self.own_dirs.iter().find(|dir| dir.starts_with(path) || path.starts_with(dir));
        if let Some(dir) = own.or_else(|| self.own_files.iter().find(|dir| dir.starts_with(path))) {
            return Some(format!(
                "{workspace} cannot be a workspace because it holds Intent's own files ({})",
                dir.display()
            ));
        }
        None
    }

    /// The entries of `config` that may be approved.
    pub fn allowed(&self, config: &WorkspacesConfig) -> WorkspacesConfig {
        WorkspacesConfig {
            workspaces: config.workspaces.iter().filter(|w| self.refusal(w).is_none()).cloned().collect(),
        }
    }
}

/// `PUT /v1/workspaces` body.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacesUpdate {
    #[serde(default)]
    pub workspaces: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::resolve_lenient;

    #[test]
    fn normalises_resolves_and_dedupes() {
        let dir = tempfile::tempdir().unwrap();
        let root = resolve_lenient(dir.path()).to_string_lossy().into_owned();
        let config = WorkspacesConfig::from_json(
            &json!({"workspaces": [root.clone(), format!("{root}/a/.."), format!("{root}/b/")]}),
        )
        .unwrap();
        assert_eq!(config.workspaces, vec![root.clone(), format!("{root}/b")]);
        assert!(config.contains(&format!("{root}/b/./")));
        assert_eq!(WorkspacesConfig::from_json(&json!({})).unwrap(), WorkspacesConfig::default());
    }

    #[test]
    fn validation_errors() {
        let err = |value: Value| WorkspacesConfig::from_json(&value).unwrap_err().to_string();
        assert_eq!(err(json!([])), "workspaces config must be an object");
        assert_eq!(err(json!({"workspaces": "/a"})), "workspaces must be a list");
        assert_eq!(err(json!({"workspaces": [" "]})), "workspace paths must be non-empty strings");
        assert_eq!(err(json!({"workspaces": [1]})), "workspace paths must be non-empty strings");
    }

    #[test]
    fn guard_refuses_broad_and_own_folders() {
        let vars = [("HOME", "/nonexistent-intent/u"), ("XDG_RUNTIME_DIR", "/nonexistent-intent/run")];
        let paths =
            Paths::from_lookup(|key| vars.iter().find(|(k, _)| *k == key).map(|(_, v)| std::ffi::OsString::from(v)));
        let guard = WorkspaceGuard::new(&paths);
        for bad in [
            "/",
            "/nonexistent-intent",
            "/nonexistent-intent/u",
            "/nonexistent-intent/u/.local",
            "/nonexistent-intent/u/.local/share/intent",
            "/nonexistent-intent/u/.local/share/intent/logs",
            "/nonexistent-intent/u/.config",
            "/nonexistent-intent/run",
        ] {
            assert!(guard.refusal(bad).is_some(), "{bad}");
        }
        assert!(guard.refusal("/nonexistent-intent/u").unwrap().contains("home directory"));
        for good in ["/nonexistent-intent/u/src", "/nonexistent-intent/u/.local/share/other", "/srv/x"] {
            assert_eq!(guard.refusal(good), None, "{good}");
        }
    }

    #[test]
    fn add_and_remove() {
        let config =
            WorkspacesConfig::default().with_added("/nonexistent-intent/w").with_added("/nonexistent-intent/w/");
        assert_eq!(config.workspaces, vec!["/nonexistent-intent/w".to_string()]);
        assert!(config.with_removed("/nonexistent-intent/x/../w").workspaces.is_empty());
    }
}
