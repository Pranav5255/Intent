//! Resume-safe markdown context for one intent (0.1 `context.py`).
//!
//! 0.1 described every failed command as "`{family}` test failed"; failures are now
//! worded by command family ("git command failed", "pytest tests failed", ...).

use intent_types::Intent;

use crate::util::{truncate_chars, unique};

pub const MAX_MARKDOWN_CHARS: usize = 2048;

pub fn build_intent_context(intent: &Intent) -> String {
    let mut lines: Vec<String> =
        vec![format!("# {}", intent.label), String::new(), intent.summary.trim().to_owned(), String::new()];
    lines.extend(resume_section(intent));
    lines.extend(insights_section(intent));
    lines.extend(todos_section(intent));
    let markdown = format!("{}\n", lines.join("\n").trim());
    if markdown.chars().count() <= MAX_MARKDOWN_CHARS {
        return markdown;
    }
    format!("{}...\n", truncate_chars(&markdown, MAX_MARKDOWN_CHARS - 3).trim_end())
}

fn resume_section(intent: &Intent) -> Vec<String> {
    let mut lines = vec!["## Resume".to_owned()];
    let payload = &intent.resume_payload;
    lines.extend(payload.files.iter().map(|p| format!("- File: {p}")));
    lines.extend(payload.urls.iter().map(|u| format!("- URL: {u}")));
    if let Some(cwd) = payload.shell.cwd.as_deref().filter(|s| !s.is_empty()) {
        let suffix = match payload.shell.last_cmd.as_deref().filter(|s| !s.is_empty()) {
            Some(cmd) => format!(" (last: {cmd})"),
            None => String::new(),
        };
        lines.push(format!("- Shell: {cwd}{suffix}"));
    }
    if lines.len() == 1 {
        lines.push("- No restore context stored.".into());
    }
    lines.push(String::new());
    lines
}

fn insights_section(intent: &Intent) -> Vec<String> {
    let mut lines = vec!["## Insights".to_owned()];
    if let Some(editor) = editor_summary(intent) {
        lines.push(format!("- Editor: {editor}"));
    }
    if let Some(browser) = browser_summary(intent) {
        lines.push(format!("- Browser: {browser}"));
    }
    if let Some(shell) = shell_summary(intent) {
        lines.push(format!("- Shell: {shell}"));
    }
    if lines.len() == 1 {
        lines.push("- No additional insights.".into());
    }
    lines.push(String::new());
    lines
}

fn todos_section(intent: &Intent) -> Vec<String> {
    if intent.todos.is_empty() {
        return Vec::new();
    }
    let mut lines = vec!["## TODOs".to_owned()];
    lines.extend(intent.todos.iter().take(5).map(|t| format!("- {} in {}", t.marker, t.path)));
    lines.push(String::new());
    lines
}

fn strings(list: &[serde_json::Map<String, serde_json::Value>], key: &str) -> Vec<String> {
    list.iter().filter_map(|i| i.get(key)?.as_str().map(str::to_owned)).collect()
}

fn editor_summary(intent: &Intent) -> Option<String> {
    let mut files = strings(&intent.insights.editor, "file");
    if files.is_empty() {
        for child in &intent.children {
            files.extend(strings(&child.insights.editor, "file"));
        }
    }
    let files = unique(files);
    match files.len() {
        0 => None,
        1 => Some(format!("heavy edits on {}", files[0])),
        _ => Some(format!("edits across {}", files.iter().take(3).cloned().collect::<Vec<_>>().join(", "))),
    }
}

fn browser_summary(intent: &Intent) -> Option<String> {
    let mut domains = strings(&intent.insights.browser, "domain");
    for child in &intent.children {
        domains.extend(strings(&child.insights.browser, "domain"));
    }
    let domains = unique(domains);
    (!domains.is_empty())
        .then(|| format!("research on {}", domains.iter().take(3).cloned().collect::<Vec<_>>().join(", ")))
}

/// How a failure of `family` reads in prose.
pub fn failure_phrase(family: &str) -> String {
    match family {
        "pytest" => "pytest tests failed".into(),
        "make" => "make target failed".into(),
        "npm" | "pnpm" | "yarn" => format!("{family} script failed"),
        "python" => "python script failed".into(),
        "pip" => "pip install failed".into(),
        "unknown" | "" => "command failed".into(),
        other => format!("{other} command failed"),
    }
}

fn shell_summary(intent: &Intent) -> Option<String> {
    let mut parts = Vec::new();
    for node in std::iter::once(intent).chain(intent.children.iter()) {
        for item in &node.insights.shell {
            let Some(family) = item.get("command_family").and_then(|v| v.as_str()) else { continue };
            let Some(count) = item.get("count").and_then(|v| v.as_i64()).filter(|c| *c > 0) else { continue };
            let times = if count > 1 { format!(" {count} times") } else { String::new() };
            let exit =
                item.get("exit_code").and_then(|v| v.as_i64()).map(|c| format!(" (exit {c})")).unwrap_or_default();
            parts.push(format!("{}{times}{exit}", failure_phrase(family)));
        }
    }
    let parts = unique(parts);
    (!parts.is_empty()).then(|| parts.into_iter().take(2).collect::<Vec<_>>().join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::intent;
    use intent_types::{ShellResume, TodoObservation};
    use serde_json::json;

    #[test]
    fn renders_sections_and_family_specific_failures() {
        let mut i = intent("a", "Debug Ledger", 0, 10);
        i.resume_payload.files = vec!["/p/a.rs".into()];
        i.resume_payload.shell = ShellResume { cwd: Some("/p".into()), last_cmd: Some("cargo test".into()) };
        i.insights.shell = vec![
            json!({"command_family": "git", "exit_code": 128, "count": 1}).as_object().unwrap().clone(),
            json!({"command_family": "pytest", "exit_code": 1, "count": 3}).as_object().unwrap().clone(),
        ];
        i.todos = vec![TodoObservation { path: "/p/a.rs".into(), observed_ts: 1, marker: "FIXME".into() }];
        let md = build_intent_context(&i);
        assert!(md.starts_with(
            "# Debug Ledger\n\nDebug Ledger.\n\n## Resume\n- File: /p/a.rs\n- Shell: /p (last: cargo test)"
        ));
        assert!(md.contains("- Shell: git command failed (exit 128); pytest tests failed 3 times (exit 1)"));
        assert!(!md.contains("git test failed"));
        assert!(md.contains("## TODOs\n- FIXME in /p/a.rs"));
        assert!(md.ends_with('\n'));
    }

    #[test]
    fn empty_sections_and_truncation() {
        let mut i = intent("a", "X", 0, 1);
        let md = build_intent_context(&i);
        assert!(md.contains("- No restore context stored."));
        assert!(md.contains("- No additional insights."));
        i.summary = "é".repeat(5000);
        let md = build_intent_context(&i);
        assert_eq!(md.chars().count(), MAX_MARKDOWN_CHARS + 1);
        assert!(md.ends_with("...\n"));
    }
}
