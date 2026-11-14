//! Credential redaction for provider-bound text (port of 1.x `redact_for_prompt`).
//!
//! Unlike 1.x, the value patterns never consume quotes, backslashes or brackets, so
//! redacting serialized JSON cannot break its structure, and nothing is truncated here
//! (bounded prompts are built at item boundaries by [`super::budget`]).

use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

fn patterns() -> &'static [(Regex, &'static str)] {
    static P: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    P.get_or_init(|| {
        let value = r#"[^\s,;"'\\\]\}\)]+"#;
        [
            (r"\bsk-[A-Za-z0-9_-]{6,}".to_owned(), "[REDACTED_KEY]"),
            (r"\b(?:gsk|xai|ghp|gho|github_pat|glpat)_[A-Za-z0-9_]{10,}".to_owned(), "[REDACTED_KEY]"),
            (r"\bAKIA[0-9A-Z]{16}\b".to_owned(), "[REDACTED_KEY]"),
            (r"\bAIza[0-9A-Za-z_-]{30,}".to_owned(), "[REDACTED_KEY]"),
            (r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]+".to_owned(), "Bearer [REDACTED_TOKEN]"),
            (format!(r"(?i)\b(password|passwd|pwd)\s*=\s*{value}"), "${1}=[REDACTED]"),
            (format!(r"(?i)\b(api[_-]?key|secret|token)\s*[:=]\s*{value}"), "${1}=[REDACTED]"),
            (
                r"-----BEGIN [A-Z ]*PRIVATE KEY-----[^-]*-----END [A-Z ]*PRIVATE KEY-----".to_owned(),
                "[REDACTED_PRIVATE_KEY]",
            ),
        ]
        .into_iter()
        .map(|(p, r)| (Regex::new(&p).expect("valid redaction regex"), r))
        .collect()
    })
}

/// Remove credential-like values from free text.
pub fn redact_text(text: &str) -> String {
    let mut value = text.to_owned();
    for (re, replacement) in patterns() {
        if re.is_match(&value) {
            value = re.replace_all(&value, *replacement).into_owned();
        }
    }
    value
}

/// Redact every string inside a JSON value in place.
pub fn redact_json(value: &mut Value) {
    match value {
        Value::String(s) => {
            let red = redact_text(s);
            if red != *s {
                *s = red;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact_json),
        Value::Object(map) => map.values_mut().for_each(redact_json),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_known_shapes() {
        let out = redact_text("key sk-abcdef123456 and Bearer abc.def password=hunter2; api_key: zzz");
        assert!(!out.contains("sk-abcdef123456"));
        assert!(!out.contains("abc.def"));
        assert!(!out.contains("hunter2"));
        assert!(!out.contains("zzz"));
        assert!(out.contains("password=[REDACTED]"));
    }

    #[test]
    fn json_structure_survives_text_redaction() {
        let doc = json!({"cmd": "export TOKEN=abc123", "pw": "password=x"}).to_string();
        let red = redact_text(&doc);
        let parsed: Value = serde_json::from_str(&red).expect("still valid JSON");
        assert_eq!(parsed["cmd"], "export TOKEN=[REDACTED]");
        assert_eq!(parsed["pw"], "password=[REDACTED]");
    }

    #[test]
    fn redact_json_values() {
        let mut v = json!({"a": ["Bearer xyz"], "b": 1});
        redact_json(&mut v);
        assert_eq!(v["a"][0], "Bearer [REDACTED_TOKEN]");
    }
}
