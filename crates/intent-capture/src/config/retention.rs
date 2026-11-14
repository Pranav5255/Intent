//! Retention windows.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{ConfigError, JsonConfig};

pub const MAX_RETENTION_DAYS: i64 = 36_500;

/// `retention-policy.json`. `None` keeps data indefinitely.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionPolicy {
    pub metadata_days: Option<i64>,
    pub detailed_days: Option<i64>,
}

impl RetentionPolicy {
    /// Human label: `indefinite`, or e.g. `metadata:30d, detailed:7d`.
    pub fn public_summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(days) = self.metadata_days {
            parts.push(format!("metadata:{days}d"));
        }
        if let Some(days) = self.detailed_days {
            parts.push(format!("detailed:{days}d"));
        }
        if parts.is_empty() {
            "indefinite".to_string()
        } else {
            parts.join(", ")
        }
    }

    /// Range check for both windows (also used by `PUT /v1/retention/policy`).
    pub fn validate(&self) -> Result<(), ConfigError> {
        for (key, value) in [
            ("metadata_days", self.metadata_days),
            ("detailed_days", self.detailed_days),
        ] {
            if let Some(days) = value
                && !(1..=MAX_RETENTION_DAYS).contains(&days)
            {
                return Err(ConfigError::invalid(format!(
                    "{key} must be a positive integer or null"
                )));
            }
        }
        Ok(())
    }
}

impl JsonConfig for RetentionPolicy {
    fn default_config() -> Self {
        RetentionPolicy::default()
    }

    fn from_json(raw: &Value) -> Result<Self, ConfigError> {
        let Value::Object(raw) = raw else {
            return Err(ConfigError::invalid("retention policy must be an object"));
        };
        let field = |key: &str| -> Result<Option<i64>, ConfigError> {
            match raw.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::Number(number)) => match number.as_i64() {
                    Some(days) if (1..=MAX_RETENTION_DAYS).contains(&days) => Ok(Some(days)),
                    _ => Err(ConfigError::invalid(format!(
                        "{key} must be a positive integer or null"
                    ))),
                },
                Some(_) => Err(ConfigError::invalid(format!(
                    "{key} must be a positive integer or null"
                ))),
            }
        };
        Ok(RetentionPolicy {
            metadata_days: field("metadata_days")?,
            detailed_days: field("detailed_days")?,
        })
    }

    fn to_json(&self) -> Value {
        json!({"metadata_days": self.metadata_days, "detailed_days": self.detailed_days})
    }

    fn decode_error(path: &Path) -> String {
        format!("invalid retention policy: {}", path.display())
    }
}

/// `POST /v1/retention/purge` body: explicit, confirmed, at least one window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionPurge {
    pub detailed_days: Option<i64>,
    pub metadata_days: Option<i64>,
    #[serde(default)]
    pub confirm: bool,
}

impl RetentionPurge {
    pub fn validate(&self) -> Result<(), ConfigError> {
        RetentionPolicy {
            metadata_days: self.metadata_days,
            detailed_days: self.detailed_days,
        }
        .validate()?;
        if self.detailed_days.is_none() && self.metadata_days.is_none() {
            return Err(ConfigError::invalid(
                "at least one retention window must be provided",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds() {
        for (value, ok) in [
            (json!(1), true),
            (json!(36_500), true),
            (json!(null), true),
            (json!(0), false),
            (json!(36_501), false),
            (json!(-5), false),
            (json!(1.0), false),
            (json!(true), false),
            (json!("30"), false),
        ] {
            let result = RetentionPolicy::from_json(&json!({"metadata_days": value}));
            assert_eq!(result.is_ok(), ok, "{value}");
            if !ok {
                assert_eq!(
                    result.unwrap_err().to_string(),
                    "metadata_days must be a positive integer or null"
                );
            }
        }
        assert_eq!(
            RetentionPolicy::from_json(&json!({"detailed_days": 0}))
                .unwrap_err()
                .to_string(),
            "detailed_days must be a positive integer or null"
        );
        assert_eq!(
            RetentionPolicy::from_json(&json!(1))
                .unwrap_err()
                .to_string(),
            "retention policy must be an object"
        );
        assert!(
            RetentionPolicy {
                metadata_days: Some(0),
                detailed_days: None
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn summary_labels() {
        assert_eq!(RetentionPolicy::default().public_summary(), "indefinite");
        assert_eq!(
            RetentionPolicy {
                metadata_days: Some(30),
                detailed_days: None
            }
            .public_summary(),
            "metadata:30d"
        );
        assert_eq!(
            RetentionPolicy {
                metadata_days: None,
                detailed_days: Some(7)
            }
            .public_summary(),
            "detailed:7d"
        );
        assert_eq!(
            RetentionPolicy {
                metadata_days: Some(30),
                detailed_days: Some(7)
            }
            .public_summary(),
            "metadata:30d, detailed:7d"
        );
    }

    #[test]
    fn purge_requires_a_window() {
        assert_eq!(
            RetentionPurge::default()
                .validate()
                .unwrap_err()
                .to_string(),
            "at least one retention window must be provided"
        );
        assert!(
            RetentionPurge {
                detailed_days: Some(7),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
        assert!(
            RetentionPurge {
                metadata_days: Some(0),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
}
