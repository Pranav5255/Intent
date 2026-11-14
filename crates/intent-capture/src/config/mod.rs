//! User-editable capture configuration files and their cached store.
//!
//! File names, JSON shapes and validation messages match 1.x. Writes are atomic
//! (tempfile + fsync + rename, mode 0600).

use std::path::Path;

use serde_json::Value;

mod detailed;
mod filesystem;
mod retention;
mod store;
mod workspaces;

pub use detailed::{
    BrowserConfig, DEFAULT_EXCLUDED_PATTERNS, DETAILED_EVENT_KINDS, DetailedCaptureBrowserUpdate,
    DetailedCaptureConfig, DetailedCaptureEditorUpdate, DetailedCaptureFilesystemUpdate, DetailedCaptureUpdate,
    DetailedKind, EditorConfig, FilesystemDetailConfig, is_detailed_event,
};
pub use filesystem::{FilesystemCaptureConfig, FilesystemCaptureUpdate};
pub use retention::{MAX_RETENTION_DAYS, RetentionPolicy, RetentionPurge};
pub use store::ConfigStore;
pub use workspaces::{WorkspacesConfig, WorkspacesUpdate};

/// Configuration error. `Invalid` maps to HTTP 422 on writes / 500 on reads (1.x).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Io(String),
}

impl ConfigError {
    pub fn invalid(message: impl Into<String>) -> Self {
        ConfigError::Invalid(message.into())
    }
}

/// A JSON config file with 1.x normalisation rules.
pub trait JsonConfig: Sized {
    /// Value used when the file does not exist.
    fn default_config() -> Self;
    /// Validate and normalise a parsed JSON document.
    fn from_json(value: &Value) -> Result<Self, ConfigError>;
    /// Canonical on-disk representation.
    fn to_json(&self) -> Value;
    /// Message for an unparsable file.
    fn decode_error(path: &Path) -> String;

    /// Load from `path`; a missing file yields [`JsonConfig::default_config`].
    fn load(path: &Path) -> Result<Self, ConfigError> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default_config());
            }
            Err(err) => {
                return Err(ConfigError::Io(format!("cannot read {}: {err}", path.display())));
            }
        };
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| ConfigError::Invalid(Self::decode_error(path)))?;
        Self::from_json(&value)
    }

    /// Re-validate, then write atomically. Returns the normalised value written.
    fn save(&self, path: &Path) -> Result<Self, ConfigError> {
        let normalised = Self::from_json(&self.to_json())?;
        crate::atomic::write_json_atomic(path, &normalised.to_json())
            .map_err(|err| ConfigError::Io(format!("cannot write {}: {err}", path.display())))?;
        Ok(normalised)
    }
}
