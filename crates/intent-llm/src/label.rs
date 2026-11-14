//! LLM label provider over privacy-safe aggregate features.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use intent_types::{COMMAND_FAMILIES, SAFE_INTENT_PRIVACY_POLICY_VERSION, SafeIntentFeatures};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::harness::{Harness, StructuredCall};
use crate::traits::{LabelLevel, LabelProvider, LabelResult, LlmError};
use crate::util::truncate_chars;

const PROMPT_VERSION: &str = "label-2.0-1";

const SYSTEM_PROMPT: &str = "You name a block of computer work from aggregate activity features. \
Return only JSON: label = 2-5 word title, summary = exactly one sentence, confidence = 0-1. \
Prefer semantic_topic, file_names, domains, project_key and command_families when present. \
Never invent file paths, URLs or commands.\n\
Example features: {\"project_key\":\"intent\",\"command_families\":[\"cargo\",\"git\"],\"file_names\":[\"harness.rs\"],\"dominant_family\":\"editor\"}\n\
Example output: {\"label\":\"Intent harness refactor\",\"summary\":\"Edited harness.rs in the intent project and ran cargo and git.\",\"confidence\":0.8}";

fn schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["label", "summary", "confidence"],
        "properties": {
            "label": {"type": "string"},
            "summary": {"type": "string"},
            "confidence": {"type": "number"}
        }
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawLabel {
    label: String,
    summary: String,
    confidence: f64,
}

/// Number of sentences: terminators that follow a letter and end the text or precede whitespace.
fn sentence_count(text: &str) -> usize {
    let chars: Vec<char> = text.trim().chars().collect();
    let mut count = 0;
    let mut i = 0;
    let mut has_content = false;
    while i < chars.len() {
        let c = chars[i];
        if matches!(c, '.' | '!' | '?') && i > 0 && chars[i - 1].is_alphabetic() {
            let mut j = i;
            while j < chars.len() && matches!(chars[j], '.' | '!' | '?') {
                j += 1;
            }
            if j == chars.len() || chars[j].is_whitespace() {
                count += 1;
                has_content = false;
                i = j;
                continue;
            }
        }
        if !c.is_whitespace() {
            has_content = true;
        }
        i += 1;
    }
    count + usize::from(has_content)
}

/// Port of 1.x `validate_label_result`.
pub fn validate_label(label: &str, summary: &str, confidence: f64) -> Result<(), String> {
    let words = label.split_whitespace().count();
    if !(2..=5).contains(&words) {
        return Err(format!("label must contain 2-5 words, got {words}"));
    }
    if summary.trim().is_empty() {
        return Err("summary must be a non-empty sentence".into());
    }
    if sentence_count(summary) != 1 {
        return Err("summary must be exactly one sentence".into());
    }
    if !(0.0..=1.0).contains(&confidence) || !confidence.is_finite() {
        return Err("confidence must be between 0 and 1".into());
    }
    Ok(())
}

/// Bound and allowlist the features once more before they leave the process.
pub fn provider_features(features: &SafeIntentFeatures) -> Value {
    let mut f = features.clone();
    f.command_families.retain(|c| COMMAND_FAMILIES.contains(&c.as_str()));
    f.command_families.truncate(5);
    f.domains.truncate(5);
    f.file_names.truncate(5);
    f.boundary_reasons.truncate(5);
    for s in f.domains.iter_mut().chain(f.file_names.iter_mut()) {
        *s = truncate_chars(s, 80).to_owned();
    }
    f.semantic_topic = f.semantic_topic.map(|t| truncate_chars(&t, 80).to_owned());
    f.project_key = f.project_key.map(|t| truncate_chars(&t, 80).to_owned());
    let mut v = serde_json::to_value(&f).unwrap_or_default();
    if let Value::Object(map) = &mut v {
        map.retain(|_, x| match x {
            Value::Null => false,
            Value::Array(a) => !a.is_empty(),
            Value::Object(o) => !o.is_empty(),
            Value::Number(n) => n.as_i64() != Some(0),
            _ => true,
        });
    }
    v
}

pub struct LlmLabelProvider {
    harness: Arc<Harness>,
    timeout: Duration,
}

impl LlmLabelProvider {
    /// `timeout` bounds the whole call including retries (defaults: 20s local, 8s cloud).
    pub fn new(harness: Arc<Harness>, timeout: Option<Duration>) -> Self {
        let local = harness.identity().is_local;
        let timeout = timeout.unwrap_or(if local { Duration::from_secs(20) } else { Duration::from_secs(8) });
        Self { harness, timeout }
    }
}

#[async_trait]
impl LabelProvider for LlmLabelProvider {
    fn cache_identity(&self) -> String {
        let id = self.harness.identity();
        format!("label:{}:{}:{SAFE_INTENT_PRIVACY_POLICY_VERSION}:{PROMPT_VERSION}", id.provider, id.model)
    }

    async fn label(&self, features: &SafeIntentFeatures, level: LabelLevel) -> Result<LabelResult, LlmError> {
        let what = match level {
            LabelLevel::Cluster => "activity",
            LabelLevel::Parent => "child-intent (a parent grouping several tasks)",
        };
        let user = format!("Safe aggregate {what} features:\n{}", provider_features(features));
        let mut call =
            StructuredCall::new("label", PROMPT_VERSION, SYSTEM_PROMPT.to_owned(), user, "intent_label", schema());
        call.max_output_tokens = 200;
        call.deadline = Some(self.timeout);
        let result = self
            .harness
            .structured::<RawLabel, _>(call, |r| validate_label(&r.label, &r.summary, r.confidence))
            .await?;
        let r = result.value;
        Ok(LabelResult {
            label: r.label.trim().to_owned(),
            summary: r.summary.trim().to_owned(),
            confidence: r.confidence,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_validation() {
        assert!(validate_label("Fix flaky tests", "Fixed flaky tests in the engine.", 0.8).is_ok());
        assert!(validate_label("Tests", "One.", 0.8).is_err());
        assert!(validate_label("a b c d e f", "One sentence.", 0.8).is_err());
        assert!(validate_label("Fix tests", "First sentence. Second sentence.", 0.8).is_err());
        assert!(validate_label("Fix tests", "Edited v2.0 config and main.rs files", 0.8).is_ok());
        assert!(validate_label("Fix tests", "Fine.", 1.2).is_err());
        assert!(validate_label("Fix tests", "  ", 0.5).is_err());
    }

    #[test]
    fn features_are_bounded() {
        let f = SafeIntentFeatures {
            policy_version: SAFE_INTENT_PRIVACY_POLICY_VERSION.into(),
            command_families: vec!["git".into(), "curl-secret".into()],
            domains: (0..9).map(|i| format!("d{i}.com")).collect(),
            ..Default::default()
        };
        let v = provider_features(&f);
        assert_eq!(v["command_families"], json!(["git"]));
        assert_eq!(v["domains"].as_array().unwrap().len(), 5);
        assert!(v.get("file_names").is_none());
        assert!(v.get("project_key").is_none());
    }
}
