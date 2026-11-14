//! Static, redacted examples of what optional LLM providers may receive
//! (port of `capture/event_server/intelligence_preview.py`,
//! `GET /v1/intelligence/preview-sample`).

use serde_json::{Value, json};

use crate::config::DetailedCaptureConfig;

pub fn build_preview_sample(config: &DetailedCaptureConfig) -> Value {
    let browser_enabled = config.browser.enabled;
    let context_enabled = config.browser.context_enabled;

    let mut packet = json!({
        "packet_id": "p0",
        "event_count": 3,
        "deterministic_cluster_id": 0,
        "timeline": [
            {"offset_s": 0, "family": "browser", "domain": "docs.python.org", "action": "click"},
            {"offset_s": 42, "family": "editor", "file": "README.md"},
            {"offset_s": 95, "family": "command", "action": "pytest"},
        ],
        "domains": ["docs.python.org"],
        "files": ["README.md"],
    });
    if context_enabled {
        packet["snippet"] = json!("Example bounded page excerpt (max 180 chars per packet).");
    }

    let mut disclosure = vec![
        "Domain names and file basenames may be sent; full paths and query strings are not.",
        "Activity counts, durations, and optional semantic topics may be sent.",
        "Page excerpts are sent only when browser detailed capture and context are enabled.",
        "Passwords, blocked domains, API keys, and full page HTML are never sent.",
    ];
    if !browser_enabled {
        disclosure.push(
            "Browser actions and excerpts are off; tab metadata stays local unless semantic full capture is enabled separately in Intelligence settings.",
        );
    }

    json!({
        "semantic_clustering_example": {"mode": "compact", "packets": [packet]},
        "labeling_example": {
            "policy_version": intent_types::SAFE_INTENT_PRIVACY_POLICY_VERSION,
            "domains": ["docs.python.org"],
            "file_names": ["README.md"],
            "dominant_family": "browser",
            "semantic_topic": "Python asyncio timeouts",
            "event_counts": {"browser": 2, "editor": 1},
            "duration_seconds": 900,
        },
        "copilot_example": {
            "question_chars_max": 2000,
            "tool_results": ["intent label", "summary", "browser domains", "semantic topic"],
            "raw_events_sent": false,
        },
        "disclosure": disclosure,
        "browser_detailed_enabled": browser_enabled,
        "browser_context_enabled": context_enabled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_sample_has_no_snippet_and_extra_disclosure() {
        let sample = build_preview_sample(&DetailedCaptureConfig::default());
        assert!(
            sample["semantic_clustering_example"]["packets"][0]
                .get("snippet")
                .is_none()
        );
        assert_eq!(sample["disclosure"].as_array().unwrap().len(), 5);
        assert_eq!(sample["browser_detailed_enabled"], false);
        assert_eq!(
            sample["labeling_example"]["policy_version"],
            "safe-intent-features-v2"
        );
        assert_eq!(sample["copilot_example"]["raw_events_sent"], false);
    }

    #[test]
    fn context_enabled_adds_snippet() {
        let mut config = DetailedCaptureConfig::default();
        config.browser.enabled = true;
        config.browser.context_enabled = true;
        let sample = build_preview_sample(&config);
        assert!(sample["semantic_clustering_example"]["packets"][0]["snippet"].is_string());
        assert_eq!(sample["disclosure"].as_array().unwrap().len(), 4);
        assert_eq!(sample["browser_context_enabled"], true);
    }
}
