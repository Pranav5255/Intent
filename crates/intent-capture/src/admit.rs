//! Ingest policy pipeline for `POST /v1/event` (the 1.x `ingest_event` handler body,
//! minus HTTP and the capture-pause check, which `intentd` owns).
//!
//! Order (unchanged from 1.x): validation → (pause) → detailed-capture consent →
//! editor workspace approval → redaction → domain blocklist.

use std::sync::Arc;

use intent_types::RawEvent;

use crate::blocklist::{Blocklist, apply_block};
use crate::config::{DetailedCaptureConfig, is_detailed_event};
use crate::redact::{RedactionError, redact_event};

/// Snapshot of the configs needed to admit events (see
/// [`crate::ConfigStore::admission_policy`]).
#[derive(Debug, Clone, Default)]
pub struct AdmissionPolicy {
    pub detailed: Arc<DetailedCaptureConfig>,
    /// Resolved approved workspace paths.
    pub approved_workspaces: Arc<Vec<String>>,
    pub blocklist: Arc<Blocklist>,
}

/// Why an otherwise valid event is silently dropped (HTTP 204 in 1.x).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DropReason {
    DetailedCaptureDisabled,
    EditorNotApproved,
}

impl DropReason {
    /// Metric label used by the 1.x ingestion metrics (`dropped_by_reason`).
    pub fn as_str(self) -> &'static str {
        match self {
            DropReason::DetailedCaptureDisabled => "detailed_capture_disabled",
            DropReason::EditorNotApproved => "editor_not_approved",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Admission {
    /// Store this (redacted, possibly blocked) event.
    Accept(RawEvent),
    /// Do not store; respond 204.
    Drop(DropReason),
}

/// Apply consent, redaction and blocking to a validated event.
///
/// `Err` means the event must be rejected (HTTP 422, metric `redaction_rejected`).
pub fn admit(event: RawEvent, policy: &AdmissionPolicy) -> Result<Admission, RedactionError> {
    let detailed = &policy.detailed;
    if is_detailed_event(&event.source, &event.kind)
        && !detailed.is_enabled(&event.source, &event.kind)
    {
        return Ok(Admission::Drop(DropReason::DetailedCaptureDisabled));
    }
    if event.source == "vscode"
        && event.kind == "document_change"
        && !detailed.editor_event_is_approved(&event.payload, &policy.approved_workspaces)
    {
        return Ok(Admission::Drop(DropReason::EditorNotApproved));
    }
    let mut event = redact_event(event)?;
    // 2.0: enforce `browser.context_enabled` server-side (1.x trusted the extension).
    if event.source == "firefox" && event.kind == "user_action" && !detailed.browser.context_enabled
    {
        event.payload.remove("context");
    }
    Ok(Admission::Accept(apply_block(event, &policy.blocklist)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DetailedKind;
    use crate::validate::{EventIn, validate_event};
    use serde_json::{Value, json};

    fn event(source: &str, kind: &str, payload: Value) -> RawEvent {
        validate_event(EventIn {
            id: "2f1c1c1e-6c3a-4c1e-9d6e-6f1f2a3b4c5d".into(),
            schema_version: 1,
            ts: 10,
            source: source.into(),
            kind: kind.into(),
            payload: payload.as_object().cloned().unwrap(),
        })
        .unwrap()
    }

    fn policy(
        configure: impl FnOnce(&mut DetailedCaptureConfig),
        workspaces: &[&str],
        blocked: &[&str],
    ) -> AdmissionPolicy {
        let mut detailed = DetailedCaptureConfig::default();
        configure(&mut detailed);
        AdmissionPolicy {
            detailed: Arc::new(detailed),
            approved_workspaces: Arc::new(workspaces.iter().map(|s| s.to_string()).collect()),
            blocklist: Arc::new(Blocklist::new(blocked).unwrap()),
        }
    }

    fn user_action() -> RawEvent {
        event(
            "firefox",
            "user_action",
            json!({"url": "https://news.example/a?x=1", "tab_id": 1, "window_id": 1, "action": "reply", "sensitive_page": false,
                   "target": {"tag": "button", "role": "button"}, "context": {"kind": "post", "text_excerpt": "hello"}}),
        )
    }

    #[test]
    fn detailed_events_need_consent() {
        let off = policy(|_| {}, &[], &[]);
        assert_eq!(
            admit(user_action(), &off).unwrap(),
            Admission::Drop(DropReason::DetailedCaptureDisabled)
        );
        let content = event(
            "filesystem",
            "file_content",
            json!({"path": "/w/a.md", "workspace": "/w", "kind": "text", "mime": "text/markdown", "size_bytes": 1, "sha256": "ab", "excerpt": "x"}),
        );
        assert_eq!(
            admit(content.clone(), &off).unwrap(),
            Admission::Drop(DropReason::DetailedCaptureDisabled)
        );
        let on = policy(|d| d.set_enabled(DetailedKind::Filesystem, true), &[], &[]);
        assert!(matches!(admit(content, &on).unwrap(), Admission::Accept(_)));
        let tab = event(
            "firefox",
            "tab_change",
            json!({"url": "https://a.com/", "title": "t"}),
        );
        assert!(matches!(admit(tab, &off).unwrap(), Admission::Accept(_)));
    }

    #[test]
    fn context_requires_context_consent() {
        let no_context = policy(|d| d.set_enabled(DetailedKind::Browser, true), &[], &[]);
        let Admission::Accept(out) = admit(user_action(), &no_context).unwrap() else {
            panic!()
        };
        assert!(out.payload.get("context").is_none());
        assert_eq!(out.payload["url"], "https://news.example/a");
        let with_context = policy(
            |d| {
                d.set_enabled(DetailedKind::Browser, true);
                d.browser.context_enabled = true;
            },
            &[],
            &[],
        );
        let Admission::Accept(out) = admit(user_action(), &with_context).unwrap() else {
            panic!()
        };
        assert_eq!(out.payload["context"]["text_excerpt"], "hello");
    }

    #[test]
    fn editor_events_need_approved_workspace() {
        let doc = event(
            "vscode",
            "document_change",
            json!({"path": "/nonexistent-intent/w/a.rs", "workspace": "/nonexistent-intent/w", "changes": [
                {"kind": "insert", "text": "password = 1", "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "removed_characters": 0}
            ]}),
        );
        let unapproved = policy(|d| d.set_enabled(DetailedKind::Editor, true), &[], &[]);
        assert_eq!(
            admit(doc.clone(), &unapproved).unwrap(),
            Admission::Drop(DropReason::EditorNotApproved)
        );
        let approved = policy(
            |d| d.set_enabled(DetailedKind::Editor, true),
            &["/nonexistent-intent/w"],
            &[],
        );
        let Admission::Accept(out) = admit(doc, &approved).unwrap() else {
            panic!()
        };
        assert_eq!(out.payload["changes"][0]["text"], "[redacted]");
    }

    #[test]
    fn redaction_errors_reject_and_blocklist_applies_last() {
        let off = policy(|_| {}, &[], &["chase.com"]);
        let bad = event(
            "firefox",
            "tab_change",
            json!({"url": "file:///etc/passwd", "title": "t"}),
        );
        assert!(admit(bad, &off).is_err());
        let blocked = event(
            "firefox",
            "tab_change",
            json!({"url": "https://user:pw@www.chase.com/acct?id=1", "title": "Chase"}),
        );
        let Admission::Accept(out) = admit(blocked, &off).unwrap() else {
            panic!()
        };
        assert_eq!(out.payload["url"], "[blocked]");
        assert_eq!(out.payload["blocked"], true);
    }

    #[test]
    fn shell_commands_are_redacted() {
        let off = policy(|_| {}, &[], &[]);
        let cmd = event(
            "shell",
            "command",
            json!({"cmd": "PGPASSWORD=x psql", "cwd": "/", "exit_code": 0}),
        );
        let Admission::Accept(out) = admit(cmd, &off).unwrap() else {
            panic!()
        };
        assert_eq!(out.payload["cmd"], "<redacted>");
        let cmd = event(
            "shell",
            "command",
            json!({"cmd": "DB=prod make deploy", "cwd": "/", "exit_code": 0}),
        );
        let Admission::Accept(out) = admit(cmd, &off).unwrap() else {
            panic!()
        };
        assert_eq!(out.payload["cmd"], "DB=<redacted> make deploy");
    }

    #[test]
    fn drop_reason_labels() {
        assert_eq!(
            DropReason::DetailedCaptureDisabled.as_str(),
            "detailed_capture_disabled"
        );
        assert_eq!(
            DropReason::EditorNotApproved.as_str(),
            "editor_not_approved"
        );
    }
}
