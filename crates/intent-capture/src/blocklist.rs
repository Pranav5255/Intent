//! Domain blocklist for browser events.
//!
//! Matching compares parsed host names, never URL substrings: `purchase.com` must
//! never match a `chase` rule.

use std::path::Path;

use intent_types::{Payload, RawEvent};
use serde_json::Value;

use crate::config::ConfigError;
use crate::url::match_host;

/// Replacement URL for blocked browser events.
pub const BLOCKED_URL: &str = "[blocked]";

/// A normalised list of blocked host patterns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Blocklist {
    patterns: Vec<String>,
}

impl Blocklist {
    /// Build from raw patterns (validated and normalised).
    pub fn new<S: AsRef<str>>(patterns: &[S]) -> Result<Self, ConfigError> {
        Ok(Blocklist { patterns: normalise_patterns(patterns)? })
    }

    /// The normalised patterns (what `GET /v1/config` reports as `blocked_domains`).
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Parse the `blocked-domains.yaml` format.
    pub fn parse_yaml(text: &str) -> Result<Self, ConfigError> {
        Self::parse_yaml_named(text, None)
    }

    fn parse_yaml_named(text: &str, path: Option<&Path>) -> Result<Self, ConfigError> {
        match parse_blocked_domains_yaml(text) {
            Ok(values) => Blocklist::new(&values),
            Err(YamlError::Structure) => Err(ConfigError::Invalid(match path {
                Some(path) => format!("invalid blocked-domains config: {}", path.display()),
                None => "invalid blocked-domains config".into(),
            })),
            Err(YamlError::Config(err)) => Err(err),
        }
    }

    /// Load from disk; a missing file is an empty blocklist.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Blocklist::parse_yaml_named(&text, Some(path)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Blocklist::default()),
            Err(err) => Err(ConfigError::Io(format!("cannot read {}: {err}", path.display()))),
        }
    }

    /// Serialise back to the YAML format (for writers such as `intentctl`).
    pub fn to_yaml(&self) -> String {
        let mut out = String::from("blocked_domains:\n");
        for pattern in &self.patterns {
            out.push_str("  - ");
            out.push_str(pattern);
            out.push('\n');
        }
        out
    }

    /// Whether the URL's host equals or is a subdomain of a rule. A rule without a
    /// dot (e.g. `chase`) also matches `chase.com` and its subdomains.
    pub fn is_url_blocked(&self, url: &str) -> bool {
        let Some(host) = match_host(url) else {
            return false;
        };
        self.patterns.iter().any(|pattern| {
            let matches = |candidate: &str| {
                host == candidate || host.strip_suffix(candidate).is_some_and(|prefix| prefix.ends_with('.'))
            };
            matches(pattern) || (!pattern.contains('.') && matches(&format!("{pattern}.com")))
        })
    }
}

enum YamlError {
    Structure,
    Config(ConfigError),
}

/// Validate and normalise host patterns: trim, lower-case, drop trailing dots and a
/// leading `*.`; reject anything with URL syntax or other wildcards. Order-preserving de-duplication.
pub fn normalise_patterns<S: AsRef<str>>(patterns: &[S]) -> Result<Vec<String>, ConfigError> {
    let mut out: Vec<String> = Vec::new();
    for pattern in patterns {
        let mut value = pattern.as_ref().trim().to_lowercase().trim_end_matches('.').to_string();
        if let Some(rest) = value.strip_prefix("*.") {
            value = rest.to_string();
        }
        if value.is_empty()
            || value.contains(['/', ':', '@', '?', '#', '[', '*'])
            || value.contains(char::is_whitespace)
        {
            return Err(ConfigError::Invalid("blocked-domains entries must be host patterns".into()));
        }
        if !out.contains(&value) {
            out.push(value);
        }
    }
    Ok(out)
}

/// Parse the small YAML subset used by `blocked-domains.yaml`:
///
/// ```yaml
/// # comment
/// blocked_domains:   # comment
///   - chase.com
///   - "wellsfargo.com"  # comment
/// ```
///
/// Also accepts `blocked_domains: []`, an inline flow list
/// (`blocked_domains: [a.com, "b.com"]`), a leading `---` document marker and CRLF.
fn parse_blocked_domains_yaml(text: &str) -> Result<Vec<String>, YamlError> {
    let invalid = || YamlError::Structure;
    let mut values: Vec<String> = Vec::new();
    let mut seen_key = false;
    let mut in_list = false;
    for raw_line in text.lines() {
        let line = strip_comment(raw_line.trim_end_matches('\r')).trim();
        if line.is_empty() || (line == "---" && !seen_key) {
            continue;
        }
        if let Some(rest) = line.strip_prefix("blocked_domains:") {
            if seen_key {
                return Err(invalid());
            }
            seen_key = true;
            let rest = rest.trim();
            if rest.is_empty() {
                in_list = true;
            } else if let Some(inner) = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                in_list = false;
                for item in inner.split(',') {
                    let item = item.trim();
                    if item.is_empty() {
                        continue;
                    }
                    values.push(unquote(item).to_string());
                }
            } else {
                return Err(invalid());
            }
            continue;
        }
        if in_list && let Some(item) = line.strip_prefix('-') {
            let value = unquote(item.trim());
            if value.is_empty() {
                return Err(YamlError::Config(ConfigError::Invalid(
                    "blocked-domains entries must be non-empty strings".into(),
                )));
            }
            values.push(value.to_string());
            continue;
        }
        return Err(invalid());
    }
    if !seen_key {
        return Err(YamlError::Config(ConfigError::Invalid(
            "blocked-domains config must contain blocked_domains".into(),
        )));
    }
    Ok(values)
}

/// Strip a `#` comment that starts the line or follows whitespace (YAML rules);
/// `#` cannot appear in a valid host anyway.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'#' && (index == 0 || bytes[index - 1].is_ascii_whitespace()) {
            return &line[..index];
        }
    }
    line
}

fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value.strip_prefix(quote).and_then(|v| v.strip_suffix(quote)) {
            return inner;
        }
    }
    value
}

/// Keep only audit-safe browser metadata for a blocked domain: URL → `[blocked]`,
/// `blocked: true`, no `context`, target reduced to `tag`/`role`, and (2.0) a
/// `title` replaced by `[blocked]` — 1.x kept the page title of blocked sites.
pub fn redact_blocked_browser_payload(payload: &Payload) -> Payload {
    let mut safe = payload.clone();
    safe.insert("url".into(), Value::String(BLOCKED_URL.into()));
    safe.insert("blocked".into(), Value::Bool(true));
    safe.remove("context");
    if safe.contains_key("title") {
        safe.insert("title".into(), Value::String(BLOCKED_URL.into()));
    }
    if let Some(Value::Object(target)) = safe.get("target") {
        let reduced: Payload = target
            .iter()
            .filter(|(key, _)| *key == "tag" || *key == "role")
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        safe.insert("target".into(), Value::Object(reduced));
    }
    safe
}

/// Browser event kinds the blocklist applies to. 2.0 adds `tab_close`
/// (1.x stored the sanitised URL of closed tabs on blocked domains).
pub const BLOCKABLE_KINDS: &[&str] = &["tab_change", "tab_close", "user_action"];

/// Apply the blocklist to a (validated, redacted) Firefox event.
pub fn apply_block(mut event: RawEvent, blocklist: &Blocklist) -> RawEvent {
    if event.source != "firefox" || !BLOCKABLE_KINDS.contains(&event.kind.as_str()) {
        return event;
    }
    let blocked = event.payload.get("url").and_then(Value::as_str).is_some_and(|url| blocklist.is_url_blocked(url));
    if blocked {
        event.payload = redact_blocked_browser_payload(&event.payload);
    }
    event
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn list(patterns: &[&str]) -> Blocklist {
        Blocklist::new(patterns).unwrap()
    }

    #[test]
    fn exact_and_subdomain_matches() {
        let b = list(&["chase.com"]);
        assert!(b.is_url_blocked("https://chase.com/"));
        assert!(b.is_url_blocked("https://www.chase.com/login"));
        assert!(b.is_url_blocked("https://a.b.CHASE.com./x"));
        assert!(b.is_url_blocked("http://user:pw@secure.chase.com:8443/"));
        assert!(!b.is_url_blocked("https://purchase.com/"));
        assert!(!b.is_url_blocked("https://chase.com.evil.net/"));
        assert!(!b.is_url_blocked("https://example.com/?r=chase.com"));
        assert!(!b.is_url_blocked("[blocked]"));
        assert!(!b.is_url_blocked(""));
    }

    #[test]
    fn no_tld_rule_matches_dot_com() {
        let b = list(&["chase"]);
        assert!(b.is_url_blocked("https://chase/"));
        assert!(b.is_url_blocked("https://chase.com/"));
        assert!(b.is_url_blocked("https://online.chase.com/"));
        assert!(!b.is_url_blocked("https://chase.org/"));
        assert!(!b.is_url_blocked("https://purchase.com/"));
    }

    #[test]
    fn patterns_are_normalised() {
        let b = list(&[" *.Example.COM. ", "example.com", "bank"]);
        assert_eq!(b.patterns(), ["example.com", "bank"]);
        for bad in
            ["", "  ", "*.", "http://x.com", "x.com/path", "user@x.com", "x.com:443", "x?y", "x#y", "[::1]", "a b"]
        {
            assert!(Blocklist::new(&[bad]).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn yaml_shipped_example() {
        let text = "# Copy to ~/.config/intent/blocked-domains.yaml\n# Rules match.\nblocked_domains:\n  - chase.com\n  - wellsfargo.com\n  - internal.company.com\n";
        assert_eq!(
            Blocklist::parse_yaml(text).unwrap().patterns(),
            ["chase.com", "wellsfargo.com", "internal.company.com"]
        );
    }

    #[test]
    fn yaml_variants() {
        let b = Blocklist::parse_yaml("---\r\nblocked_domains: # c\r\n- \"a.com\" # x\r\n-   'B.com'\r\n\r\n").unwrap();
        assert_eq!(b.patterns(), ["a.com", "b.com"]);
        assert!(Blocklist::parse_yaml("blocked_domains: []\n").unwrap().is_empty());
        assert!(Blocklist::parse_yaml("blocked_domains:\n").unwrap().is_empty());
        let b = Blocklist::parse_yaml("blocked_domains: [a.com, \"b.com\", 'c']").unwrap();
        assert_eq!(b.patterns(), ["a.com", "b.com", "c"]);
    }

    #[test]
    fn yaml_errors() {
        let err = |text: &str| Blocklist::parse_yaml(text).unwrap_err().to_string();
        assert_eq!(err(""), "blocked-domains config must contain blocked_domains");
        assert_eq!(err("# only comments\n"), "blocked-domains config must contain blocked_domains");
        assert_eq!(err("blocked_domains:\n  -\n"), "blocked-domains entries must be non-empty strings");
        assert_eq!(err("blocked_domains:\n  - https://x.com/\n"), "blocked-domains entries must be host patterns");
        assert!(Blocklist::parse_yaml("- a.com\nblocked_domains:\n").is_err());
        assert!(Blocklist::parse_yaml("other: 1\nblocked_domains:\n").is_err());
        assert!(Blocklist::parse_yaml("blocked_domains: a.com\n").is_err());
        assert!(Blocklist::parse_yaml("blocked_domains:\nblocked_domains:\n").is_err());
    }

    #[test]
    fn load_reports_path_and_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blocked-domains.yaml");
        assert!(Blocklist::load(&path).unwrap().is_empty());
        std::fs::write(&path, "nonsense: true\n").unwrap();
        assert_eq!(
            Blocklist::load(&path).unwrap_err().to_string(),
            format!("invalid blocked-domains config: {}", path.display())
        );
        let b = list(&["a.com", "b"]);
        std::fs::write(&path, b.to_yaml()).unwrap();
        assert_eq!(Blocklist::load(&path).unwrap(), b);
    }

    fn firefox(kind: &str, payload: Value) -> RawEvent {
        RawEvent {
            id: "x".into(),
            ts: 1,
            source: "firefox".into(),
            kind: kind.into(),
            payload: payload.as_object().cloned().unwrap(),
            schema_version: 1,
            ingested_at: None,
        }
    }

    #[test]
    fn apply_block_redacts_user_action() {
        let b = list(&["chase.com"]);
        let event = firefox(
            "user_action",
            json!({"url": "https://www.chase.com/acct", "tab_id": 1, "window_id": 1, "action": "click", "sensitive_page": false,
                   "target": {"tag": "a", "role": "link", "label": "Pay", "href": "https://chase.com/pay"},
                   "context": {"kind": "k", "text_excerpt": "balance"}}),
        );
        let out = apply_block(event, &b);
        assert_eq!(out.payload["url"], "[blocked]");
        assert_eq!(out.payload["blocked"], true);
        assert!(out.payload.get("context").is_none());
        assert_eq!(out.payload["target"], json!({"tag": "a", "role": "link"}));
        assert_eq!(out.payload["tab_id"], 1);
    }

    #[test]
    fn apply_block_tab_events() {
        let b = list(&["chase.com"]);
        let out = apply_block(firefox("tab_change", json!({"url": "https://chase.com/x", "title": "Accounts"})), &b);
        assert_eq!(
            out.payload,
            json!({"url": "[blocked]", "title": "[blocked]", "blocked": true}).as_object().cloned().unwrap()
        );
        let out = apply_block(firefox("tab_close", json!({"url": "https://chase.com/x"})), &b);
        assert_eq!(out.payload["url"], "[blocked]");
        let untouched = firefox("tab_change", json!({"url": "https://example.com/", "title": "t"}));
        assert_eq!(apply_block(untouched.clone(), &b), untouched);
        let mut other = firefox("tab_change", json!({"url": "https://chase.com/"}));
        other.source = "linux".into();
        assert_eq!(apply_block(other.clone(), &b), other);
    }
}
