//! Bounded restore payloads (0.1 `resume.py`).

use std::collections::HashSet;

use intent_types::{Family, NormalizedEvent, RESUME_MAX_FILES, RESUME_MAX_URLS, ResumePayload, ShellResume};
use serde_json::Value;

pub const RESUME_POLICY_VERSION: &str = "2";

/// Newest-first restore payload: ≤5 files, ≤8 URLs (final URL of each open tab;
/// closed tabs and closed URLs skipped), and the newest command's shell context.
pub fn build_resume_payload(cluster: &[&NormalizedEvent]) -> ResumePayload {
    let mut ordered: Vec<&NormalizedEvent> = cluster.to_vec();
    ordered.sort_by_key(|e| std::cmp::Reverse((e.ts, e.ordinal)));

    let mut files: Vec<String> = Vec::new();
    let mut urls: Vec<String> = Vec::new();
    let mut seen_files: HashSet<&str> = HashSet::new();
    let mut seen_urls: HashSet<&str> = HashSet::new();
    let mut seen_tabs: HashSet<i64> = HashSet::new();
    let mut closed_tabs: HashSet<i64> = HashSet::new();
    let mut closed_urls: HashSet<&str> = HashSet::new();
    let mut shell: Option<ShellResume> = None;

    for event in ordered {
        match event.family {
            Family::Editor => {
                if let Some(path) = event.entities.file_path.as_deref()
                    && files.len() < RESUME_MAX_FILES
                    && seen_files.insert(path)
                {
                    files.push(path.to_owned());
                }
            }
            Family::Browser => {
                let url = event_url(event);
                let tab = event_tab_id(event);
                if event.category == "tab_close" {
                    closed_tabs.extend(tab);
                    closed_urls.extend(url);
                    continue;
                }
                if let Some(tab) = tab {
                    if seen_tabs.contains(&tab) || closed_tabs.contains(&tab) {
                        continue;
                    }
                    // Mark the tab even when its final URL is shared with another tab,
                    // so an older URL from this tab is not restored instead.
                    seen_tabs.insert(tab);
                }
                if let Some(url) = url
                    && restorable_domain(url).is_some()
                    && !seen_urls.contains(url)
                    && !closed_urls.contains(url)
                    && urls.len() < RESUME_MAX_URLS
                {
                    seen_urls.insert(url);
                    urls.push(url.to_owned());
                }
            }
            Family::Command
                if shell.is_none() && (event.entities.cwd.is_some() || event.entities.command.is_some()) =>
            {
                shell = Some(ShellResume { cwd: event.entities.cwd.clone(), last_cmd: event.entities.command.clone() });
            }
            _ => {}
        }
    }
    ResumePayload { files, urls, shell: shell.unwrap_or_default() }
}

/// Merge chronological payloads, treating the last as newest.
pub fn merge_resume_payloads(payloads: &[ResumePayload]) -> ResumePayload {
    let mut out = ResumePayload::default();
    for payload in payloads.iter().rev() {
        for path in &payload.files {
            if out.files.len() < RESUME_MAX_FILES && !out.files.contains(path) {
                out.files.push(path.clone());
            }
        }
        for url in &payload.urls {
            if out.urls.len() < RESUME_MAX_URLS && !out.urls.contains(url) && restorable_domain(url).is_some() {
                out.urls.push(url.clone());
            }
        }
        if out.shell.cwd.is_none() {
            out.shell.cwd = payload.shell.cwd.clone().filter(|s| !s.is_empty());
        }
        if out.shell.last_cmd.is_none() {
            out.shell.last_cmd = payload.shell.last_cmd.clone().filter(|s| !s.is_empty());
        }
    }
    out
}

fn payload_field<'a>(event: &'a NormalizedEvent, key: &str) -> Option<&'a Value> {
    event.raw.get("payload").and_then(Value::as_object).and_then(|p| p.get(key))
}

fn event_url(event: &NormalizedEvent) -> Option<&str> {
    payload_field(event, "url").and_then(Value::as_str)
}

fn event_tab_id(event: &NormalizedEvent) -> Option<i64> {
    match payload_field(event, "tab_id") {
        Some(Value::Number(n)) => n.as_i64().filter(|t| *t >= 0),
        _ => None,
    }
}

/// Lower-cased host of an `http(s)` URL that is safe to reopen.
pub fn restorable_domain(url: &str) -> Option<String> {
    if url.to_lowercase().contains("[redacted]") {
        return None;
    }
    let (scheme, rest) = url.split_once("://")?;
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if let Some(stripped) = host_port.strip_prefix('[') {
        stripped.split_once(']')?.0
    } else {
        host_port.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::normalize::normalize_event;
    use crate::test_support::{cmd, edit, raw};
    use serde_json::json;

    fn tab(id: &str, ts: i64, kind: &str, url: &str, tab: i64) -> NormalizedEvent {
        normalize_event(&raw(id, ts, "firefox", kind, json!({"url": url, "tab_id": tab})), ts as usize)
    }

    #[test]
    fn final_url_per_tab_and_closed_tabs_skipped() {
        let events = [
            tab("a", 1, "tab_change", "https://a.com/1", 1),
            tab("b", 2, "tab_change", "https://a.com/2", 1),
            tab("c", 3, "tab_change", "https://b.com/", 2),
            tab("d", 4, "tab_close", "https://b.com/", 2),
            tab("e", 5, "tab_change", "https://[redacted].com/", 3),
            tab("f", 6, "tab_change", "file:///etc/passwd", 4),
        ];
        let refs: Vec<&NormalizedEvent> = events.iter().collect();
        assert_eq!(build_resume_payload(&refs).urls, vec!["https://a.com/2"]);
    }

    #[test]
    fn caps_files_and_takes_newest_shell() {
        let mut events: Vec<NormalizedEvent> =
            (0..7).map(|i| edit(&format!("e{i}"), i, &format!("/p/f{i}.py"), "/p")).collect();
        events.push(cmd("c1", 10, "pytest", "/p", 1));
        events.push(cmd("c2", 11, "git status", "/p/sub", 0));
        let refs: Vec<&NormalizedEvent> = events.iter().collect();
        let p = build_resume_payload(&refs);
        assert_eq!(p.files, vec!["/p/f6.py", "/p/f5.py", "/p/f4.py", "/p/f3.py", "/p/f2.py"]);
        assert_eq!(p.shell.last_cmd.as_deref(), Some("git status"));
        assert_eq!(p.shell.cwd.as_deref(), Some("/p/sub"));
    }

    #[test]
    fn url_cap() {
        let events: Vec<NormalizedEvent> =
            (0..12).map(|i| tab(&format!("t{i}"), i, "tab_change", &format!("https://s{i}.com/"), i)).collect();
        let refs: Vec<&NormalizedEvent> = events.iter().collect();
        assert_eq!(build_resume_payload(&refs).urls.len(), RESUME_MAX_URLS);
    }

    #[test]
    fn merge_newest_first() {
        let a = ResumePayload {
            files: vec!["/a".into()],
            urls: vec!["https://a.com/".into()],
            shell: ShellResume { cwd: Some("/a".into()), last_cmd: None },
        };
        let b = ResumePayload {
            files: vec!["/b".into(), "/a".into()],
            urls: vec![],
            shell: ShellResume { cwd: None, last_cmd: Some("ls".into()) },
        };
        let m = merge_resume_payloads(&[a, b]);
        assert_eq!(m.files, vec!["/b", "/a"]);
        assert_eq!(m.shell.cwd.as_deref(), Some("/a"));
        assert_eq!(m.shell.last_cmd.as_deref(), Some("ls"));
    }

    #[test]
    fn restorable_hosts() {
        assert_eq!(restorable_domain("HTTPS://User@Docs.RS:443/x").as_deref(), Some("docs.rs"));
        assert_eq!(restorable_domain("http://[::1]:80/").as_deref(), Some("::1"));
        assert_eq!(restorable_domain("about:blank"), None);
        assert_eq!(restorable_domain("https:///x"), None);
    }
}
