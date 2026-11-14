//! Event builders for unit tests.

use intent_types::{NormalizedEvent, RawEvent};
use serde_json::{Value, json};

use crate::normalize::normalize_event;

pub fn raw(id: &str, ts: i64, source: &str, kind: &str, payload: Value) -> RawEvent {
    RawEvent {
        id: id.into(),
        ts,
        source: source.into(),
        kind: kind.into(),
        payload: payload.as_object().cloned().unwrap_or_default(),
        schema_version: 1,
        ingested_at: None,
    }
}

pub fn ev(id: &str, ts: i64, source: &str, kind: &str) -> NormalizedEvent {
    normalize_event(&raw(id, ts, source, kind, json!({})), 0)
}

pub fn cmd(id: &str, ts: i64, command: &str, cwd: &str, exit_code: i64) -> NormalizedEvent {
    normalize_event(&raw(id, ts, "shell", "command", json!({"cmd": command, "cwd": cwd, "exit_code": exit_code})), 0)
}

pub fn edit(id: &str, ts: i64, path: &str, workspace: &str) -> NormalizedEvent {
    normalize_event(&raw(id, ts, "vscode", "document_change", json!({"path": path, "workspace": workspace})), 0)
}

pub fn browse(id: &str, ts: i64, url: &str) -> NormalizedEvent {
    normalize_event(&raw(id, ts, "firefox", "tab_change", json!({"url": url})), 0)
}

pub fn focus(id: &str, ts: i64, title: &str) -> NormalizedEvent {
    normalize_event(&raw(id, ts, "linux", "app_focus", json!({"title": title})), 0)
}

pub fn intent(id: &str, label: &str, start_ts: i64, end_ts: i64) -> intent_types::Intent {
    intent_types::Intent {
        id: id.into(),
        parent_id: None,
        date: "2026-01-01".into(),
        label: label.into(),
        summary: format!("{label}."),
        confidence: 0.5,
        start_ts,
        end_ts,
        depth: 0,
        tags: vec![],
        stats: intent_types::IntentStats { event_count: 1, duration_seconds: end_ts - start_ts, ..Default::default() },
        insights: Default::default(),
        todos: vec![],
        evidence: vec![],
        resume_payload: Default::default(),
        prefix: None,
        semantic: None,
        privacy_policy_version: "safe-intent-features-v2".into(),
        children: vec![],
    }
}

/// Re-number ordinals so builders can be used for whole sessions.
pub fn seq(mut events: Vec<NormalizedEvent>) -> Vec<NormalizedEvent> {
    for (i, e) in events.iter_mut().enumerate() {
        e.ordinal = i;
    }
    events
}
