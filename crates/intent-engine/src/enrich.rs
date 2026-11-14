//! Stats, insights, TODOs, project tags and tree validation (1.x `enrich.py`).

use std::collections::BTreeMap;

use intent_types::{Family, Intent, IntentStats, NormalizedEvent, TodoObservation};
use serde_json::{Map, Value, json};

use crate::normalize::inserted_texts;
use crate::util::{OrderedCounter, final_segment, parent_path, unique};

/// Vote for a project name from normalized paths only (`project:<segment>`).
pub fn derive_project_tag(cluster: &[&NormalizedEvent]) -> Option<String> {
    let mut segments = OrderedCounter::new();
    for event in cluster {
        let e = &event.entities;
        let mut candidates: Vec<String> = e.project_paths.clone();
        if let Some(cwd) = &e.cwd {
            candidates.push(cwd.clone());
        }
        if event.family == Family::Editor
            && let Some(parent) = e.file_path.as_deref().and_then(parent_path)
        {
            candidates.push(parent);
        }
        for path in candidates {
            if let Some(segment) = final_segment(&path) {
                segments.add(&segment);
            }
        }
    }
    segments.top().map(|s| format!("project:{s}"))
}

pub fn compute_stats(cluster: &[&NormalizedEvent]) -> IntentStats {
    let (Some(first), Some(last)) = (cluster.first(), cluster.last()) else {
        return IntentStats::default();
    };
    let mut sources: BTreeMap<String, i64> = BTreeMap::new();
    for event in cluster {
        *sources.entry(event.source.clone()).or_insert(0) += 1;
    }
    IntentStats {
        event_count: cluster.len() as i64,
        duration_seconds: last.ts - first.ts,
        sources,
        unique_apps: unique(cluster.iter().map(|e| e.source.clone())),
    }
}

/// Sum child stats in child order (durations are summed, as in 1.x).
pub fn aggregate_stats(children: &[Intent]) -> IntentStats {
    let mut stats = IntentStats::default();
    for child in children {
        stats.event_count += child.stats.event_count;
        stats.duration_seconds += child.stats.duration_seconds;
        for (source, count) in &child.stats.sources {
            *stats.sources.entry(source.clone()).or_insert(0) += count;
        }
        for app in &child.stats.unique_apps {
            if !stats.unique_apps.contains(app) {
                stats.unique_apps.push(app.clone());
            }
        }
    }
    stats
}

/// Invariants for a depth-0 intent and its direct children; returns the violations
/// (1.x asserted, which crashed the whole run).
pub fn validate_intent_tree(root: &Intent) -> Vec<String> {
    let mut problems = Vec::new();
    if root.depth != 0 {
        problems.push("root intent must have depth 0".into());
    }
    if root.children.iter().any(|c| c.depth != 1) {
        problems.push("children must have depth 1".into());
    }
    if root.children.iter().any(|c| c.parent_id.as_deref() != Some(root.id.as_str())) {
        problems.push("children must reference root".into());
    }
    if !root.children.is_empty()
        && root.children.iter().map(|c| c.stats.event_count).sum::<i64>() != root.stats.event_count
    {
        problems.push("event counts must aggregate".into());
    }
    if root.children.iter().any(|c| c.start_ts < root.start_ts) {
        problems.push("root must start before children".into());
    }
    if root.children.iter().any(|c| c.end_ts > root.end_ts) {
        problems.push("root must end after children".into());
    }
    problems
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// Editor activity without document text: dominant file, typed chars, saves of it.
pub fn compute_insight_editor(cluster: &[&NormalizedEvent]) -> Vec<Map<String, Value>> {
    let editor: Vec<&&NormalizedEvent> = cluster.iter().filter(|e| e.family == Family::Editor).collect();
    if editor.is_empty() {
        return Vec::new();
    }
    let typed: i64 = editor.iter().map(|e| e.signals.typed_chars).sum();
    let files: OrderedCounter = editor.iter().filter_map(|e| e.entities.file_name.as_deref()).collect();
    let dominant = files.top();
    let saves = match dominant {
        Some(file) => editor.iter().filter(|e| e.signals.save && e.entities.file_name.as_deref() == Some(file)).count(),
        None => 0,
    };
    vec![object(json!({"file": dominant, "typed_chars": typed, "saves": saves}))]
}

/// Top three browser domains by visits (ties: first seen).
pub fn compute_insight_browser(cluster: &[&NormalizedEvent]) -> Vec<Map<String, Value>> {
    let domains: OrderedCounter =
        cluster.iter().filter(|e| e.family == Family::Browser).filter_map(|e| e.entities.domain.as_deref()).collect();
    domains.by_frequency().into_iter().take(3).map(|(d, n)| object(json!({"domain": d, "visits": n}))).collect()
}

/// Failed command families with exit codes, most frequent first; no command text.
pub fn compute_insight_shell(cluster: &[&NormalizedEvent]) -> Vec<Map<String, Value>> {
    let mut order: Vec<(String, i64)> = Vec::new();
    let mut counts: Vec<usize> = Vec::new();
    for event in cluster.iter().filter(|e| e.family == Family::Command) {
        let Some(code) = event.entities.exit_code.filter(|c| *c != 0) else { continue };
        let key = (event.entities.command_family.clone().unwrap_or_else(|| "unknown".into()), code);
        match order.iter().position(|k| *k == key) {
            Some(i) => counts[i] += 1,
            None => {
                order.push(key);
                counts.push(1);
            }
        }
    }
    let mut ranked: Vec<usize> = (0..order.len()).collect();
    ranked.sort_by(|&a, &b| counts[b].cmp(&counts[a]).then(a.cmp(&b)));
    ranked
        .into_iter()
        .map(|i| object(json!({"command_family": order[i].0, "exit_code": order[i].1, "count": counts[i]})))
        .collect()
}

/// TODO/FIXME/XXX observations from inserted editor text.
///
/// The marker comes from the inserted text itself (1.x searched the event headline,
/// which never contains it, so every observation was reported as `TODO`).
pub fn detect_todos(cluster: &[&NormalizedEvent]) -> Vec<TodoObservation> {
    let mut out = Vec::new();
    for event in cluster {
        if event.family != Family::Editor || event.category != "document_change" || !event.signals.todo_added {
            continue;
        }
        let Some(path) = &event.entities.file_path else { continue };
        let payload = event.raw.get("payload").and_then(Value::as_object);
        let marker = payload.and_then(|p| inserted_texts(p).find_map(find_marker)).unwrap_or("TODO");
        out.push(TodoObservation { path: path.clone(), observed_ts: event.ts, marker: marker.into() });
    }
    out
}

/// First whole-word TODO / FIXME / XXX (case-insensitive) in `text`.
fn find_marker(text: &str) -> Option<&'static str> {
    let upper = text.to_ascii_uppercase();
    let bytes = upper.as_bytes();
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut best: Option<(usize, &'static str)> = None;
    for marker in ["TODO", "FIXME", "XXX"] {
        let mut from = 0;
        while let Some(pos) = upper[from..].find(marker) {
            let start = from + pos;
            let end = start + marker.len();
            let before_ok = start == 0 || !is_word(bytes[start - 1]);
            let after_ok = end == bytes.len() || !is_word(bytes[end]);
            if before_ok && after_ok {
                if best.is_none_or(|(b, _)| start < b) {
                    best = Some((start, marker));
                }
                break;
            }
            from = start + 1;
        }
    }
    best.map(|(_, m)| m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::normalize::normalize_event;
    use crate::test_support::{browse, cmd, edit, raw, seq};

    fn refs(e: &[NormalizedEvent]) -> Vec<&NormalizedEvent> {
        e.iter().collect()
    }

    #[test]
    fn project_tag_votes_on_final_segment() {
        let s = seq(vec![
            edit("a", 0, "/home/u/alpha/src/app.ts", "/home/u/alpha"),
            cmd("b", 1, "npm test", "/home/u/beta", 0),
        ]);
        // alpha: workspace + nothing from file parent "src"; beta: cwd counted twice (path + cwd).
        assert_eq!(derive_project_tag(&refs(&s)).as_deref(), Some("project:beta"));
        assert_eq!(derive_project_tag(&[]), None);
    }

    #[test]
    fn stats_and_aggregation() {
        let s = seq(vec![edit("a", 10, "/p/a.py", "/p"), browse("b", 70, "https://x.com/")]);
        let stats = compute_stats(&refs(&s));
        assert_eq!(stats.event_count, 2);
        assert_eq!(stats.duration_seconds, 60);
        assert_eq!(stats.unique_apps, vec!["vscode", "firefox"]);
    }

    #[test]
    fn insights() {
        let s = seq(vec![
            cmd("a", 0, "pytest", "/p", 1),
            cmd("b", 1, "git push", "/p", 128),
            cmd("c", 2, "pytest", "/p", 1),
            cmd("d", 3, "ls", "/p", 2),
            cmd("e", 4, "pytest", "/p", 0),
            browse("f", 5, "https://a.com/"),
            browse("g", 6, "https://b.com/"),
            browse("h", 7, "https://b.com/x"),
        ]);
        let shell = compute_insight_shell(&refs(&s));
        assert_eq!(shell[0]["command_family"], "pytest");
        assert_eq!(shell[0]["count"], 2);
        assert_eq!(shell[2]["command_family"], "unknown");
        let browser = compute_insight_browser(&refs(&s));
        assert_eq!(browser[0]["domain"], "b.com");
        assert_eq!(browser[1]["visits"], 1);
        assert!(compute_insight_editor(&refs(&s)).is_empty());
    }

    #[test]
    fn todo_marker_comes_from_inserted_text() {
        let e = normalize_event(
            &raw(
                "t",
                5,
                "vscode",
                "document_change",
                serde_json::json!({"path": "/p/a.rs", "changes": [{"text": "// xxx: later; FIXME"}]}),
            ),
            0,
        );
        let todos = detect_todos(&[&e]);
        assert_eq!(todos.len(), 1);
        assert_eq!(todos[0].marker, "XXX");
        assert_eq!(find_marker("TODOS are not markers"), None);
        assert_eq!(find_marker("fixme now"), Some("FIXME"));
    }

    #[test]
    fn tree_validation_reports_instead_of_panicking() {
        let s = seq(vec![edit("a", 10, "/p/a.py", "/p")]);
        let mut root = Intent {
            id: "r".into(),
            parent_id: None,
            date: "d".into(),
            label: "l".into(),
            summary: "s".into(),
            confidence: 0.5,
            start_ts: 10,
            end_ts: 10,
            depth: 0,
            tags: vec![],
            stats: compute_stats(&refs(&s)),
            insights: Default::default(),
            todos: vec![],
            evidence: vec![],
            resume_payload: Default::default(),
            prefix: None,
            semantic: None,
            privacy_policy_version: "x".into(),
            children: vec![],
        };
        let mut child = root.clone();
        child.id = "c".into();
        child.depth = 1;
        child.parent_id = Some("r".into());
        child.end_ts = 99;
        root.children.push(child);
        let problems = validate_intent_tree(&root);
        assert_eq!(problems, vec!["root must end after children".to_owned()]);
    }
}
