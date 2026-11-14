//! Tolerant JSON extraction from model text.
//!
//! Handles reasoning blocks (`<think>…</think>`), Markdown code fences and surrounding
//! prose. Candidates are found with a balanced-brace scanner that understands JSON
//! strings, so `{"a":"}"}` and prose like "use {x} here" do not confuse it (1.x used
//! first-`{`/last-`}`, which fails on any brace in trailing prose).

use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

/// Remove reasoning-model scratchpads.
pub fn strip_reasoning(text: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?is)<(think|thinking|reasoning|reflection)>.*?</(think|thinking|reasoning|reflection)>")
            .expect("valid regex")
    });
    let mut out = re.replace_all(text, "").into_owned();
    // A dangling close tag (server stripped the opener): keep what follows the last one.
    let lower = out.to_lowercase();
    if let Some(pos) =
        ["</think>", "</thinking>", "</reasoning>"].iter().filter_map(|t| lower.rfind(t).map(|p| p + t.len())).max()
    {
        out = out[pos..].to_owned();
    }
    // An unterminated opener (output cut off mid-thought): drop the scratchpad.
    let lower = out.to_lowercase();
    if let Some(pos) = ["<think>", "<thinking>", "<reasoning>"].iter().filter_map(|t| lower.find(t)).min() {
        out.truncate(pos);
    }
    out
}

/// All JSON values found in `text`, most plausible first: the whole text, fenced
/// blocks, then balanced `{…}` / `[…]` spans in order of appearance.
pub fn json_candidates(text: &str) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let push = |v: Value, out: &mut Vec<Value>| {
        if !out.contains(&v) {
            out.push(v);
        }
    };
    let stripped = strip_reasoning(text);
    for source in [stripped.as_str(), text] {
        let trimmed = source.trim();
        if let Ok(v) = serde_json::from_str::<Value>(trimmed)
            && (v.is_object() || v.is_array())
        {
            push(v, &mut out);
        }
        for block in fenced_blocks(trimmed) {
            if let Ok(v) = serde_json::from_str::<Value>(block.trim()) {
                push(v, &mut out);
            }
        }
        for span in balanced_spans(trimmed) {
            if let Ok(v) = serde_json::from_str::<Value>(span) {
                push(v, &mut out);
            }
        }
        if !out.is_empty() {
            break;
        }
    }
    out
}

/// First JSON object/array in `text`.
pub fn extract_json(text: &str) -> Result<Value, String> {
    json_candidates(text).into_iter().next().ok_or_else(|| "the reply did not contain a JSON object".to_owned())
}

fn fenced_blocks(text: &str) -> Vec<&str> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?s)```[A-Za-z0-9_-]*[ \t]*\r?\n?(.*?)```").expect("valid regex"));
    re.captures_iter(text).filter_map(|c| c.get(1).map(|m| m.as_str())).collect()
}

/// Top-level balanced bracket spans, honouring JSON string escapes.
fn balanced_spans(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let mut start = 0;
    while start < bytes.len() {
        let Some(offset) = bytes[start..].iter().position(|b| *b == b'{' || *b == b'[') else { break };
        let open = start + offset;
        match match_close(bytes, open) {
            Some(close) => {
                let span = &text[open..=close];
                if serde_json::from_str::<Value>(span).is_ok() {
                    spans.push(span);
                    start = close + 1;
                    continue;
                }
                start = open + 1;
            }
            None => start = open + 1,
        }
    }
    spans
}

fn match_close(bytes: &[u8], open: usize) -> Option<usize> {
    let mut stack: Vec<u8> = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        if in_string {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => stack.push(b'}'),
            b'[' => stack.push(b']'),
            b'}' | b']' => {
                if stack.pop() != Some(b) {
                    return None;
                }
                if stack.is_empty() {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plain_json() {
        assert_eq!(extract_json(r#" {"a":1} "#).unwrap(), json!({"a": 1}));
    }

    #[test]
    fn code_fence() {
        let text = "Here you go:\n```json\n{\"a\": [1, 2]}\n```\nThanks!";
        assert_eq!(extract_json(text).unwrap(), json!({"a": [1, 2]}));
    }

    #[test]
    fn think_block_with_braces() {
        let text = "<think>maybe {\"a\": 0} or {b}?</think>\n{\"a\": 2}";
        assert_eq!(extract_json(text).unwrap(), json!({"a": 2}));
    }

    #[test]
    fn dangling_think_close() {
        let text = "reasoning about {x}...</think>{\"ok\":true}";
        assert_eq!(extract_json(text).unwrap(), json!({"ok": true}));
    }

    #[test]
    fn prose_with_trailing_brace() {
        // first-{ / last-} would produce invalid JSON here.
        let text = "Result: {\"label\": \"Fix {bug}\", \"n\": 1} -- note: {not json}";
        assert_eq!(extract_json(text).unwrap(), json!({"label": "Fix {bug}", "n": 1}));
    }

    #[test]
    fn escaped_quotes_in_strings() {
        let text = r#"x {"s": "a \"}\" b"} y"#;
        assert_eq!(extract_json(text).unwrap(), json!({"s": "a \"}\" b"}));
    }

    #[test]
    fn multiple_candidates_in_order() {
        let c = json_candidates("first {\"a\":1} then {\"b\":2}");
        assert_eq!(c, vec![json!({"a": 1}), json!({"b": 2})]);
    }

    #[test]
    fn no_json() {
        assert!(extract_json("I cannot help with that.").is_err());
        assert!(extract_json("{\"unterminated\": ").is_err());
    }

    #[test]
    fn json_only_inside_unterminated_think_is_still_found() {
        let c = json_candidates("<think>{\"a\":1}");
        assert_eq!(c, vec![json!({"a": 1})]);
    }
}
