//! Token/character budgets. Prompts are bounded by dropping whole items, never by
//! slicing serialized JSON (0.1 cut prompts at 4,000 chars mid-structure).

use serde_json::Value;

/// Rough token estimate (~4 chars per token for English/JSON; conservative for code).
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// Serialized compact JSON length of a value.
pub fn json_len(value: &Value) -> usize {
    serde_json::to_string(value).map(|s| s.len()).unwrap_or(0)
}

/// Keep the longest prefix of `items` whose JSON array encoding fits `max_chars`.
/// Returns the kept items and how many were dropped.
pub fn fit_items(items: Vec<Value>, max_chars: usize) -> (Vec<Value>, usize) {
    let mut used = 2; // []
    let mut kept = Vec::with_capacity(items.len());
    let total = items.len();
    for item in items {
        let cost = json_len(&item) + usize::from(!kept.is_empty());
        if used + cost > max_chars {
            break;
        }
        used += cost;
        kept.push(item);
    }
    let dropped = total - kept.len();
    (kept, dropped)
}

/// Bound free text at a char boundary, preferring a whitespace boundary.
pub fn bound_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let cut = crate::util::truncate_chars(text, max_chars);
    match cut.rfind(char::is_whitespace) {
        Some(pos) if pos > max_chars / 2 => cut[..pos].to_owned(),
        _ => cut.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fit_items_stops_at_item_boundary() {
        let items = vec![json!({"a": "xxxx"}), json!({"b": "yyyy"}), json!({"c": "zzzz"})];
        let one = json_len(&items[0]);
        let (kept, dropped) = fit_items(items.clone(), 2 + one * 2 + 1);
        assert_eq!(kept.len(), 2);
        assert_eq!(dropped, 1);
        let encoded = serde_json::to_string(&kept).unwrap();
        assert!(encoded.len() <= 2 + one * 2 + 1);
        assert!(serde_json::from_str::<Value>(&encoded).is_ok());
    }

    #[test]
    fn bound_text_prefers_words() {
        assert_eq!(bound_text("hello brave new world", 12), "hello brave");
        assert_eq!(estimate_tokens("abcdefgh"), 2);
    }
}
