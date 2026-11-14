//! Broad filesystem observation consent (port of `capture/tools/filesystem_capture.py`).

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{ConfigError, JsonConfig};

/// `filesystem-capture.json`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesystemCaptureConfig {
    pub all_accessible: bool,
}

impl JsonConfig for FilesystemCaptureConfig {
    fn default_config() -> Self {
        FilesystemCaptureConfig::default()
    }

    fn from_json(raw: &Value) -> Result<Self, ConfigError> {
        let invalid = || ConfigError::invalid("invalid filesystem capture configuration");
        let Value::Object(raw) = raw else {
            return Err(invalid());
        };
        match raw.get("all_accessible") {
            None => Ok(FilesystemCaptureConfig::default()),
            Some(Value::Bool(all_accessible)) => Ok(FilesystemCaptureConfig {
                all_accessible: *all_accessible,
            }),
            Some(_) => Err(invalid()),
        }
    }

    fn to_json(&self) -> Value {
        json!({"all_accessible": self.all_accessible})
    }

    fn decode_error(path: &Path) -> String {
        format!(
            "invalid filesystem capture configuration: {}",
            path.display()
        )
    }

    fn load(path: &Path) -> Result<Self, ConfigError> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default_config());
            }
            Err(err) => {
                return Err(ConfigError::Io(format!(
                    "cannot read {}: {err}",
                    path.display()
                )));
            }
        };
        serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|value| Self::from_json(&value).ok())
            .ok_or_else(|| ConfigError::Invalid(Self::decode_error(path)))
    }
}

/// `PUT /v1/filesystem-capture` body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesystemCaptureUpdate {
    pub all_accessible: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rules() {
        assert!(
            !FilesystemCaptureConfig::from_json(&json!({}))
                .unwrap()
                .all_accessible
        );
        assert!(
            FilesystemCaptureConfig::from_json(&json!({"all_accessible": true, "x": 1}))
                .unwrap()
                .all_accessible
        );
        assert!(FilesystemCaptureConfig::from_json(&json!({"all_accessible": 1})).is_err());
        assert!(FilesystemCaptureConfig::from_json(&json!(true)).is_err());
    }

    #[test]
    fn load_error_names_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("filesystem-capture.json");
        std::fs::write(&path, "{\"all_accessible\": \"yes\"}").unwrap();
        assert_eq!(
            FilesystemCaptureConfig::load(&path)
                .unwrap_err()
                .to_string(),
            format!(
                "invalid filesystem capture configuration: {}",
                path.display()
            )
        );
    }
}
