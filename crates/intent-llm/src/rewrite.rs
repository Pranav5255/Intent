//! Natural-language question → 1–5 short FTS keyword queries.

use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::harness::redact::redact_text;
use crate::harness::{Harness, StructuredCall};
use crate::util::truncate_chars;

pub const MAX_REWRITE_QUERIES: usize = 5;
pub const MAX_QUERY_LENGTH: usize = 100;
const PROMPT_VERSION: &str = "rewrite-2.0-1";

const SYSTEM_PROMPT: &str = "Rewrite the user's question about their past computer activity into 1 to 5 short full-text search \
keyword queries (2-4 words each, no operators, no dates). Return only JSON like {\"queries\":[\"terraform vpc\",\"aws networking\"]}.";

#[derive(Debug, Serialize, Deserialize)]
struct Queries {
    queries: Vec<String>,
}

fn schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["queries"],
        "properties": {"queries": {"type": "array", "minItems": 1, "maxItems": MAX_REWRITE_QUERIES, "items": {"type": "string"}}}
    })
}

/// Strip FTS syntax characters and secrets; bound length.
pub fn sanitize_query(value: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"["'`*?:;(){}\[\]^~\\]"#).expect("valid regex"));
    let red = redact_text(value);
    let cleaned = re.replace_all(&red, " ");
    let squashed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&squashed, MAX_QUERY_LENGTH).trim().to_owned()
}

/// Returns 1–5 sanitized queries; falls back to the sanitized question on any failure.
pub async fn rewrite_search_queries(question: &str, harness: &Harness, timeout: Duration) -> Vec<String> {
    let original = sanitize_query(question);
    if original.is_empty() {
        return vec![question.trim().to_owned()];
    }
    let mut call = StructuredCall::new(
        "rewrite",
        PROMPT_VERSION,
        SYSTEM_PROMPT.to_owned(),
        truncate_chars(question, 2000).to_owned(),
        "search_queries",
        schema(),
    );
    call.max_output_tokens = 200;
    call.deadline = Some(timeout);
    let Ok(result) = harness
        .structured::<Queries, _>(call, |q| {
            if q.queries.iter().any(|s| !sanitize_query(s).is_empty()) {
                Ok(())
            } else {
                Err("queries must not be empty".into())
            }
        })
        .await
    else {
        return vec![original];
    };
    let mut cleaned: Vec<String> = Vec::new();
    for q in result.value.queries.iter().take(MAX_REWRITE_QUERIES) {
        let s = sanitize_query(q);
        if !s.is_empty() && !cleaned.contains(&s) {
            cleaned.push(s);
        }
    }
    if cleaned.is_empty() { vec![original] } else { cleaned }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize() {
        assert_eq!(sanitize_query("what (was) \"I\" doing*?  "), "what was I doing");
        assert!(!sanitize_query("token=abc123 fix").contains("abc123"));
        assert_eq!(sanitize_query(&"x".repeat(300)).len(), MAX_QUERY_LENGTH);
    }
}
