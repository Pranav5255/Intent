//! Approved workspaces (port of `capture/tools/workspaces.py`).

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{ConfigError, JsonConfig};
use crate::paths::resolve_user_path;

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
        WorkspacesConfig {
            workspaces: self
                .workspaces
                .iter()
                .filter(|item| **item != resolved)
                .cloned()
                .collect(),
        }
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
                return Err(ConfigError::invalid(
                    "workspace paths must be non-empty strings",
                ));
            };
            let path = resolve_user_path(path);
            if !resolved.contains(&path) {
                resolved.push(path);
            }
        }
        Ok(WorkspacesConfig {
            workspaces: resolved,
        })
    }

    fn to_json(&self) -> Value {
        json!({"workspaces": self.workspaces})
    }

    fn decode_error(path: &Path) -> String {
        format!("invalid workspaces config: {}", path.display())
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
        assert_eq!(
            WorkspacesConfig::from_json(&json!({})).unwrap(),
            WorkspacesConfig::default()
        );
    }

    #[test]
    fn validation_errors() {
        let err = |value: Value| WorkspacesConfig::from_json(&value).unwrap_err().to_string();
        assert_eq!(err(json!([])), "workspaces config must be an object");
        assert_eq!(
            err(json!({"workspaces": "/a"})),
            "workspaces must be a list"
        );
        assert_eq!(
            err(json!({"workspaces": [" "]})),
            "workspace paths must be non-empty strings"
        );
        assert_eq!(
            err(json!({"workspaces": [1]})),
            "workspace paths must be non-empty strings"
        );
    }

    #[test]
    fn add_and_remove() {
        let config = WorkspacesConfig::default()
            .with_added("/nonexistent-intent/w")
            .with_added("/nonexistent-intent/w/");
        assert_eq!(config.workspaces, vec!["/nonexistent-intent/w".to_string()]);
        assert!(
            config
                .with_removed("/nonexistent-intent/x/../w")
                .workspaces
                .is_empty()
        );
    }
}
