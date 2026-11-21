//! Strict ingest validation.
//!
//! Error messages match 1.x so companions and logs read the same.

use std::collections::BTreeSet;
use std::fmt;

use intent_types::{Payload, RawEvent};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

/// Supported `(source, type)` pairs and their required payload fields, in 1.x order.
pub const EVENT_PAYLOAD_FIELDS: &[((&str, &str), &[&str])] = &[
    (("linux", "app_focus"), &["app", "title"]),
    (("linux", "idle_start"), &[]),
    (("linux", "idle_end"), &[]),
    (("firefox", "tab_change"), &["url", "title"]),
    (("firefox", "tab_close"), &["url"]),
    (("firefox", "user_action"), &["url", "tab_id", "window_id", "action", "target", "sensitive_page"]),
    (("vscode", "workspace_open"), &["folder"]),
    (("vscode", "file_open"), &["path"]),
    (("vscode", "file_edit"), &["path"]),
    (("vscode", "file_save"), &["path"]),
    (("vscode", "document_change"), &["path", "workspace", "changes"]),
    (("filesystem", "file_modify"), &["path", "workspace"]),
    (("filesystem", "workspace_seen"), &["workspace"]),
    (("filesystem", "file_content"), &["path", "workspace", "kind", "mime", "size_bytes", "sha256", "excerpt"]),
    (("shell", "command"), &["cmd", "cwd", "exit_code"]),
];

/// How far ahead of the caller's clock an event `ts` may be. A day covers clock skew
/// between a companion and the daemon. A later `ts` is a bug (often milliseconds sent
/// as seconds): it never reaches a day view and sticks to the current intent.
pub const MAX_FUTURE_SKEW_SECONDS: i64 = 86_400;

pub const MAX_DOCUMENT_CHANGES: usize = 25;
pub const MAX_DOCUMENT_TEXT_BYTES: usize = 8 * 1024;
pub const CHANGE_KINDS: &[&str] = &["insert", "delete", "replace"];
pub const USER_ACTIONS: &[&str] = &[
    "click",
    "link_activation",
    "form_submit",
    "toggle",
    "select_change",
    "scroll",
    "like",
    "reply",
    "repost",
    "share",
    "follow",
    "unfollow",
];
pub const FILE_CONTENT_KINDS: &[&str] = &["text", "pdf", "image"];

/// An event as posted by a collector (`POST /v1/event`), before validation.
///
/// Integer fields accept JSON integers, integral floats and decimal strings
/// (pydantic "lax" coercion); booleans are rejected. Unknown fields are ignored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventIn {
    pub id: String,
    #[serde(default = "default_schema_version", deserialize_with = "lax_int")]
    pub schema_version: i64,
    #[serde(deserialize_with = "lax_int")]
    pub ts: i64,
    pub source: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub payload: Payload,
}

fn default_schema_version() -> i64 {
    1
}

fn lax_int<'de, D: Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    use serde::de::Error;
    match Value::deserialize(deserializer)? {
        Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                return Ok(value);
            }
            match number.as_f64() {
                Some(value) if value.fract() == 0.0 && value.abs() < 9.0e18 => Ok(value as i64),
                _ => Err(D::Error::custom("value is not a valid integer")),
            }
        }
        Value::String(text) => text.trim().parse::<i64>().map_err(|_| D::Error::custom("value is not a valid integer")),
        _ => Err(D::Error::custom("value is not a valid integer")),
    }
}

/// A rejected event; `message` is safe to return to the collector (HTTP 422).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub message: String,
}

impl ValidationError {
    pub fn new(message: impl Into<String>) -> Self {
        ValidationError { message: message.into() }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ValidationError {}

type VResult = Result<(), ValidationError>;

fn fail<T>(message: impl Into<String>) -> Result<T, ValidationError> {
    Err(ValidationError::new(message))
}

/// Required payload fields for a supported event kind.
pub fn required_fields(source: &str, kind: &str) -> Option<&'static [&'static str]> {
    EVENT_PAYLOAD_FIELDS.iter().find(|((s, k), _)| *s == source && *k == kind).map(|(_, fields)| *fields)
}

/// Validate and normalise a posted event into the stored shape.
///
/// `source`/`type` are trimmed and lower-cased; `id` is canonicalised to the
/// hyphenated lower-case UUID form. `now` (UTC Unix seconds) bounds `ts` from above.
pub fn validate_event(event: EventIn, now: i64) -> Result<RawEvent, ValidationError> {
    let id = Uuid::try_parse(event.id.trim()).map_err(|_| ValidationError::new("id must be a valid UUID"))?;
    if !(1..=99).contains(&event.schema_version) {
        return fail("schema_version must be between 1 and 99");
    }
    if event.ts < 0 {
        return fail("ts must be a non-negative integer (UTC Unix seconds)");
    }
    if event.ts > now.saturating_add(MAX_FUTURE_SKEW_SECONDS) {
        return fail("ts must not be more than 1 day in the future (UTC Unix seconds, not milliseconds)");
    }
    let source_len = event.source.chars().count();
    if !(1..=32).contains(&source_len) {
        return fail("source must be 1 to 32 characters");
    }
    let kind_len = event.kind.chars().count();
    if !(1..=64).contains(&kind_len) {
        return fail("type must be 1 to 64 characters");
    }
    let source = event.source.trim().to_lowercase();
    let kind = event.kind.trim().to_lowercase();
    validate_payload(&source, &kind, &event.payload)?;
    Ok(RawEvent {
        id: id.hyphenated().to_string(),
        ts: event.ts,
        source,
        kind,
        payload: event.payload,
        schema_version: event.schema_version,
        ingested_at: None,
    })
}

/// Validate a payload for an already-normalised `(source, kind)`.
pub fn validate_payload(source: &str, kind: &str, payload: &Payload) -> VResult {
    let Some(required) = required_fields(source, kind) else {
        let supported: Vec<String> = EVENT_PAYLOAD_FIELDS.iter().map(|((s, k), _)| format!("{s}/{k}")).collect();
        return fail(format!("unsupported event kind {source}/{kind}; supported: {}", supported.join(", ")));
    };
    let missing: Vec<&str> = required.iter().copied().filter(|field| !payload.contains_key(*field)).collect();
    if !missing.is_empty() {
        return fail(format!("payload missing required fields: {}", missing.join(", ")));
    }
    for field in required {
        if payload[*field].is_null() {
            return fail(format!("payload field {field} cannot be null"));
        }
    }
    match (source, kind) {
        ("vscode", "document_change") => validate_document_change(payload),
        ("firefox", "user_action") => validate_user_action(payload),
        ("filesystem", "file_content") => validate_file_content(payload),
        ("shell", "command") => validate_shell_command(payload),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn keys(object: &Map<String, Value>) -> BTreeSet<&str> {
    object.keys().map(String::as_str).collect()
}

fn has_only(object: &Map<String, Value>, allowed: &[&str]) -> bool {
    object.keys().all(|key| allowed.contains(&key.as_str()))
}

fn keys_exactly(object: &Map<String, Value>, wanted: &[&str]) -> bool {
    keys(object) == wanted.iter().copied().collect()
}

fn bounded_string(value: Option<&Value>, field: &str, maximum: usize) -> VResult {
    match value {
        Some(Value::String(text)) if !text.is_empty() => {
            if text.chars().count() > maximum {
                fail(format!("payload {field} must be at most {maximum} characters"))
            } else {
                Ok(())
            }
        }
        _ => fail(format!("payload {field} must be a non-empty string")),
    }
}

/// JSON integer `>= 0` (booleans and floats are not integers).
fn is_non_negative_int(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Number(number)) if number.is_u64())
}

fn position(value: Option<&Value>, field: &str) -> VResult {
    let Some(Value::Object(object)) = value else {
        return fail(format!("payload {field} must contain line and character"));
    };
    if !keys_exactly(object, &["line", "character"]) {
        return fail(format!("payload {field} must contain line and character"));
    }
    if !object.values().all(|item| is_non_negative_int(Some(item))) {
        return fail(format!("payload {field} coordinates must be non-negative integers"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// strict validators
// ---------------------------------------------------------------------------

fn validate_document_change(payload: &Payload) -> VResult {
    if !has_only(payload, &["path", "workspace", "language", "changes"]) {
        return fail("document_change payload contains unsupported fields");
    }
    bounded_string(payload.get("path"), "path", 4096)?;
    bounded_string(payload.get("workspace"), "workspace", 4096)?;
    if payload.contains_key("language") {
        bounded_string(payload.get("language"), "language", 64)?;
    }
    let changes = match payload.get("changes") {
        Some(Value::Array(items)) if (1..=MAX_DOCUMENT_CHANGES).contains(&items.len()) => items,
        _ => {
            return fail(format!("payload changes must contain 1 to {MAX_DOCUMENT_CHANGES} items"));
        }
    };
    let mut total_bytes = 0usize;
    for (index, change) in changes.iter().enumerate() {
        let Value::Object(change) = change else {
            return fail(format!("payload changes[{index}] must be an object"));
        };
        let required = ["kind", "range", "removed_characters"];
        let allowed = ["kind", "range", "removed_characters", "text", "text_length", "redacted"];
        if !required.iter().all(|key| change.contains_key(*key)) || !has_only(change, &allowed) {
            return fail(format!("payload changes[{index}] has invalid fields"));
        }
        let kind = change.get("kind").and_then(Value::as_str);
        if !kind.is_some_and(|kind| CHANGE_KINDS.contains(&kind)) {
            return fail(format!("payload changes[{index}].kind is invalid"));
        }
        let range = match change.get("range") {
            Some(Value::Object(range)) if keys_exactly(range, &["start", "end"]) => range,
            _ => {
                return fail(format!("payload changes[{index}].range must contain start and end"));
            }
        };
        position(range.get("start"), &format!("changes[{index}].range.start"))?;
        position(range.get("end"), &format!("changes[{index}].range.end"))?;
        if !is_non_negative_int(change.get("removed_characters")) {
            return fail(format!("payload changes[{index}].removed_characters must be a non-negative integer"));
        }
        // An explicit `"text": null` is treated like an absent text (1.x `dict.get`).
        let text = change.get("text").filter(|value| !value.is_null());
        if matches!(kind, Some("insert" | "replace")) && !matches!(text, Some(Value::String(_))) {
            return fail(format!("payload changes[{index}].text is required for insert and replace"));
        }
        match text {
            Some(Value::String(text)) => total_bytes += text.len(),
            Some(_) => return fail(format!("payload changes[{index}].text must be a string")),
            None => {}
        }
        if change.contains_key("text_length") && !is_non_negative_int(change.get("text_length")) {
            return fail(format!("payload changes[{index}].text_length must be a non-negative integer"));
        }
        if change.contains_key("redacted") && !matches!(change.get("redacted"), Some(Value::Bool(_))) {
            return fail(format!("payload changes[{index}].redacted must be a boolean"));
        }
    }
    if total_bytes > MAX_DOCUMENT_TEXT_BYTES {
        return fail(format!("payload changes text must be at most {MAX_DOCUMENT_TEXT_BYTES} bytes"));
    }
    Ok(())
}

fn optional_bounded_target_field(target: &Map<String, Value>, field: &str, maximum: usize) -> VResult {
    match target.get(field) {
        None => Ok(()),
        Some(Value::String(text)) if text.chars().count() <= maximum => Ok(()),
        Some(_) => fail(format!("payload target.{field} must be a string of at most {maximum} characters")),
    }
}

fn validate_user_action(payload: &Payload) -> VResult {
    let allowed = ["url", "tab_id", "window_id", "action", "target", "sensitive_page", "scroll", "context", "blocked"];
    if !has_only(payload, &allowed) {
        return fail("user_action payload contains unsupported fields");
    }
    bounded_string(payload.get("url"), "url", 4096)?;
    if !is_non_negative_int(payload.get("tab_id")) {
        return fail("payload tab_id must be a non-negative integer");
    }
    if !is_non_negative_int(payload.get("window_id")) {
        return fail("payload window_id must be a non-negative integer");
    }
    let action = payload.get("action").and_then(Value::as_str).filter(|action| USER_ACTIONS.contains(action));
    let Some(action) = action else {
        return fail("payload action is invalid");
    };
    let Some(Value::Bool(sensitive_page)) = payload.get("sensitive_page") else {
        return fail("payload sensitive_page must be a boolean");
    };
    if payload.contains_key("blocked") && !matches!(payload.get("blocked"), Some(Value::Bool(_))) {
        return fail("payload blocked must be a boolean");
    }
    let target = match payload.get("target") {
        Some(Value::Object(target)) if target.contains_key("tag") && target.contains_key("role") => target,
        _ => return fail("payload target must contain tag and role"),
    };
    if !has_only(target, &["tag", "role", "label", "input_type", "href", "checked"]) {
        return fail("payload target contains unsupported fields");
    }
    bounded_string(target.get("tag"), "target.tag", 64)?;
    if !matches!(target.get("role"), Some(Value::String(role)) if role.chars().count() <= 64) {
        return fail("payload target.role must be a string of at most 64 characters");
    }
    if *sensitive_page {
        if action != "click" && action != "form_submit" {
            return fail("sensitive-page actions must be click or form_submit");
        }
        if !keys_exactly(target, &["tag", "role"]) {
            return fail("sensitive-page targets may contain only tag and role");
        }
        // 2.0: 1.x returned early here, so a sensitive-page event could still carry
        // a page excerpt or scroll data. The extension never sends them; reject.
        if payload.contains_key("context") {
            return fail("sensitive-page payload may not include context");
        }
        if payload.contains_key("scroll") {
            return fail("payload scroll is valid only for scroll actions");
        }
        return Ok(());
    }
    if action == "scroll" {
        let is_document = keys_exactly(target, &["tag", "role"])
            && target.get("tag").and_then(Value::as_str) == Some("document")
            && target.get("role").and_then(Value::as_str) == Some("document");
        if !is_document {
            return fail("scroll target must be the document");
        }
        let scroll = match payload.get("scroll") {
            Some(Value::Object(scroll)) if keys_exactly(scroll, &["direction", "position_bucket"]) => scroll,
            _ => return fail("payload scroll must contain direction and position_bucket"),
        };
        if !matches!(scroll.get("direction").and_then(Value::as_str), Some("up" | "down")) {
            return fail("payload scroll.direction is invalid");
        }
        let bucket_ok = is_non_negative_int(scroll.get("position_bucket"))
            && scroll.get("position_bucket").and_then(Value::as_u64).is_some_and(|bucket| bucket <= 10);
        if !bucket_ok {
            return fail("payload scroll.position_bucket must be an integer from 0 to 10");
        }
        if payload.contains_key("context") {
            return fail("scroll payload may not include context");
        }
    } else if payload.contains_key("scroll") {
        return fail("payload scroll is valid only for scroll actions");
    }
    optional_bounded_target_field(target, "label", 160)?;
    optional_bounded_target_field(target, "input_type", 64)?;
    optional_bounded_target_field(target, "href", 4096)?;
    if target.contains_key("checked") && !matches!(target.get("checked"), Some(Value::Bool(_))) {
        return fail("payload target.checked must be a boolean");
    }
    if let Some(context) = payload.get("context") {
        let context = match context {
            Value::Object(context) if has_only(context, &["kind", "author", "text_excerpt"]) => context,
            _ => return fail("payload context has invalid fields"),
        };
        bounded_string(context.get("kind"), "context.kind", 32)?;
        if context.contains_key("author") {
            bounded_string(context.get("author"), "context.author", 160)?;
        }
        bounded_string(context.get("text_excerpt"), "context.text_excerpt", 1000)?;
    }
    Ok(())
}

fn validate_file_content(payload: &Payload) -> VResult {
    let allowed = ["path", "workspace", "kind", "mime", "size_bytes", "sha256", "excerpt"];
    if !has_only(payload, &allowed) {
        return fail("file_content payload contains unsupported fields");
    }
    for (field, maximum) in
        [("path", 4096), ("workspace", 4096), ("kind", 16), ("mime", 128), ("sha256", 64), ("excerpt", 4000)]
    {
        bounded_string(payload.get(field), field, maximum)?;
    }
    if !payload.get("kind").and_then(Value::as_str).is_some_and(|kind| FILE_CONTENT_KINDS.contains(&kind)) {
        return fail("file_content payload kind is invalid");
    }
    if !is_non_negative_int(payload.get("size_bytes")) {
        return fail("file_content payload size_bytes must be a non-negative integer");
    }
    Ok(())
}

/// 2.0: shell commands are redacted server-side, which requires `cmd` to be a string
/// (1.x accepted any JSON value, so a list could bypass the sender's redaction).
fn validate_shell_command(payload: &Payload) -> VResult {
    if !matches!(payload.get("cmd"), Some(Value::String(_))) {
        return fail("payload cmd must be a string");
    }
    if !matches!(payload.get("cwd"), Some(Value::String(_))) {
        return fail("payload cwd must be a string");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ID: &str = "2f1c1c1e-6c3a-4c1e-9d6e-6f1f2a3b4c5d";
    const NOW: i64 = 1_700_000_000;

    fn event(source: &str, kind: &str, payload: Value) -> EventIn {
        EventIn {
            id: ID.into(),
            schema_version: 1,
            ts: NOW,
            source: source.into(),
            kind: kind.into(),
            payload: payload.as_object().cloned().unwrap(),
        }
    }

    fn err(source: &str, kind: &str, payload: Value) -> String {
        validate_event(event(source, kind, payload), NOW).unwrap_err().message
    }

    fn ok(source: &str, kind: &str, payload: Value) -> RawEvent {
        validate_event(event(source, kind, payload), NOW).unwrap()
    }

    // ---- envelope ---------------------------------------------------------

    #[test]
    fn parses_python_shape_with_lax_ints() {
        let json = format!(
            r#"{{"id":"{ID}","ts":"12","schema_version":2.0,"source":"shell","type":"command","payload":{{}},"extra":1}}"#
        );
        let parsed: EventIn = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.ts, 12);
        assert_eq!(parsed.schema_version, 2);
        let json = format!(r#"{{"id":"{ID}","ts":1,"source":"s","type":"t"}}"#);
        let parsed: EventIn = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.schema_version, 1);
        assert!(parsed.payload.is_empty());
        for bad in [r#""ts":true"#, r#""ts":1.5"#, r#""ts":"x""#, r#""ts":null"#] {
            let json = format!(r#"{{"id":"{ID}",{bad},"source":"s","type":"t"}}"#);
            assert!(serde_json::from_str::<EventIn>(&json).is_err(), "{bad}");
        }
    }

    #[test]
    fn id_must_be_uuid_and_is_canonicalised() {
        let mut e = event("linux", "idle_start", json!({}));
        e.id = "2F1C1C1E6C3A4C1E9D6E6F1F2A3B4C5D".into();
        assert_eq!(validate_event(e.clone(), NOW).unwrap().id, ID);
        e.id = "{2f1c1c1e-6c3a-4c1e-9d6e-6f1f2a3b4c5d}".into();
        assert_eq!(validate_event(e.clone(), NOW).unwrap().id, ID);
        e.id = "not-a-uuid".into();
        assert_eq!(validate_event(e, NOW).unwrap_err().message, "id must be a valid UUID");
    }

    #[test]
    fn schema_version_bounds() {
        for (version, valid) in [(0, false), (1, true), (99, true), (100, false), (-1, false)] {
            let mut e = event("linux", "idle_start", json!({}));
            e.schema_version = version;
            assert_eq!(validate_event(e, NOW).is_ok(), valid, "{version}");
        }
    }

    #[test]
    fn ts_must_be_non_negative() {
        let mut e = event("linux", "idle_start", json!({}));
        e.ts = 0;
        assert!(validate_event(e.clone(), NOW).is_ok());
        e.ts = -1;
        assert!(validate_event(e, NOW).unwrap_err().message.starts_with("ts must be"));
    }

    #[test]
    fn ts_must_not_be_far_in_the_future() {
        let mut e = event("linux", "idle_start", json!({}));
        for (ts, valid) in [
            (NOW, true),
            (NOW + 30, true),
            (NOW + MAX_FUTURE_SKEW_SECONDS, true),
            (NOW + MAX_FUTURE_SKEW_SECONDS + 1, false),
            (NOW + 400 * 86_400, false),
            (NOW * 1000, false),
            (i64::MAX, false),
        ] {
            e.ts = ts;
            let result = validate_event(e.clone(), NOW);
            assert_eq!(result.is_ok(), valid, "{ts}");
            if let Err(err) = result {
                assert!(err.message.contains("in the future"), "{}", err.message);
            }
        }
        // The bound moves with the clock the caller passes in.
        e.ts = NOW * 1000;
        assert!(validate_event(e, NOW * 1000).is_ok());
    }

    #[test]
    fn source_and_type_lengths_and_normalisation() {
        let raw = ok(" Linux ", "IDLE_START", json!({}));
        assert_eq!((raw.source.as_str(), raw.kind.as_str()), ("linux", "idle_start"));
        assert_eq!(err("", "idle_start", json!({})), "source must be 1 to 32 characters");
        assert_eq!(err(&"a".repeat(33), "x", json!({})), "source must be 1 to 32 characters");
        assert_eq!(err("linux", "", json!({})), "type must be 1 to 64 characters");
        assert_eq!(err("linux", &"a".repeat(65), json!({})), "type must be 1 to 64 characters");
        assert!(err(&"a".repeat(32), &"b".repeat(64), json!({})).starts_with("unsupported event kind"));
    }

    #[test]
    fn unsupported_kind_lists_supported() {
        let message = err("chrome", "tab_change", json!({}));
        assert!(
            message
                .starts_with("unsupported event kind chrome/tab_change; supported: linux/app_focus, linux/idle_start")
        );
        assert!(message.ends_with("shell/command"));
    }

    #[test]
    fn required_and_null_fields() {
        assert_eq!(err("linux", "app_focus", json!({"app": "x"})), "payload missing required fields: title");
        assert_eq!(err("linux", "app_focus", json!({})), "payload missing required fields: app, title");
        assert_eq!(err("linux", "app_focus", json!({"app": null, "title": "t"})), "payload field app cannot be null");
        let raw = ok("linux", "app_focus", json!({"app": "x", "title": "", "extra": 1}));
        assert_eq!(raw.payload["extra"], 1);
        ok("firefox", "tab_change", json!({"url": "https://a/", "title": "t"}));
        ok("firefox", "tab_close", json!({"url": "https://a/"}));
        ok("vscode", "workspace_open", json!({"folder": "/w"}));
        ok("vscode", "file_save", json!({"path": "/w/a"}));
        ok("filesystem", "file_modify", json!({"path": "/w/a", "workspace": "/w"}));
        ok("filesystem", "workspace_seen", json!({"workspace": "/w"}));
    }

    #[test]
    fn every_kind_is_reachable() {
        for ((source, kind), _) in EVENT_PAYLOAD_FIELDS {
            assert!(required_fields(source, kind).is_some());
        }
        assert_eq!(EVENT_PAYLOAD_FIELDS.len(), 15);
    }

    // ---- shell ------------------------------------------------------------

    #[test]
    fn shell_command_requires_string_cmd_and_cwd() {
        ok("shell", "command", json!({"cmd": "ls", "cwd": "/", "exit_code": 0, "duration_ms": 3}));
        assert_eq!(
            err("shell", "command", json!({"cmd": ["ls"], "cwd": "/", "exit_code": 0})),
            "payload cmd must be a string"
        );
        assert_eq!(
            err("shell", "command", json!({"cmd": "ls", "cwd": 1, "exit_code": 0})),
            "payload cwd must be a string"
        );
    }

    // ---- document_change --------------------------------------------------

    fn change(kind: &str, text: Option<&str>) -> Value {
        let mut value = json!({
            "kind": kind,
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
            "removed_characters": 0,
        });
        if let Some(text) = text {
            value["text"] = json!(text);
        }
        value
    }

    fn doc(changes: Vec<Value>) -> Value {
        json!({"path": "/w/a.rs", "workspace": "/w", "language": "rust", "changes": changes})
    }

    fn doc_err(changes: Vec<Value>) -> String {
        err("vscode", "document_change", doc(changes))
    }

    #[test]
    fn document_change_valid() {
        ok(
            "vscode",
            "document_change",
            doc(vec![change("insert", Some("x")), change("delete", None), change("replace", Some(""))]),
        );
        let mut c = change("insert", Some("x"));
        c["text_length"] = json!(1);
        c["redacted"] = json!(false);
        ok("vscode", "document_change", doc(vec![c]));
        let mut c = change("delete", None);
        c["text"] = Value::Null;
        ok("vscode", "document_change", doc(vec![c]));
    }

    #[test]
    fn document_change_payload_fields() {
        let mut payload = doc(vec![change("insert", Some("x"))]);
        payload["extra"] = json!(1);
        assert_eq!(err("vscode", "document_change", payload), "document_change payload contains unsupported fields");
        let mut payload = doc(vec![change("insert", Some("x"))]);
        payload["path"] = json!("");
        assert_eq!(err("vscode", "document_change", payload), "payload path must be a non-empty string");
        let mut payload = doc(vec![change("insert", Some("x"))]);
        payload["path"] = json!("a".repeat(4097));
        assert_eq!(err("vscode", "document_change", payload), "payload path must be at most 4096 characters");
        let mut payload = doc(vec![change("insert", Some("x"))]);
        payload["workspace"] = json!(5);
        assert_eq!(err("vscode", "document_change", payload), "payload workspace must be a non-empty string");
        let mut payload = doc(vec![change("insert", Some("x"))]);
        payload["language"] = json!("l".repeat(65));
        assert_eq!(err("vscode", "document_change", payload), "payload language must be at most 64 characters");
        let mut payload = doc(vec![change("insert", Some("x"))]);
        payload.as_object_mut().unwrap().remove("language");
        ok("vscode", "document_change", payload);
    }

    #[test]
    fn document_change_counts() {
        assert_eq!(doc_err(vec![]), "payload changes must contain 1 to 25 items");
        assert_eq!(doc_err(vec![change("delete", None); 26]), "payload changes must contain 1 to 25 items");
        ok("vscode", "document_change", doc(vec![change("delete", None); 25]));
        let mut payload = doc(vec![]);
        payload["changes"] = json!({"kind": "insert"});
        assert_eq!(err("vscode", "document_change", payload), "payload changes must contain 1 to 25 items");
    }

    #[test]
    fn document_change_items() {
        assert_eq!(doc_err(vec![json!("x")]), "payload changes[0] must be an object");
        let mut c = change("insert", Some("x"));
        c.as_object_mut().unwrap().remove("range");
        assert_eq!(doc_err(vec![c]), "payload changes[0] has invalid fields");
        let mut c = change("insert", Some("x"));
        c["other"] = json!(1);
        assert_eq!(doc_err(vec![change("delete", None), c]), "payload changes[1] has invalid fields");
        assert_eq!(doc_err(vec![change("move", None)]), "payload changes[0].kind is invalid");
        let mut c = change("insert", Some("x"));
        c["range"] = json!({"start": {"line": 0, "character": 0}});
        assert_eq!(doc_err(vec![c]), "payload changes[0].range must contain start and end");
        let mut c = change("insert", Some("x"));
        c["range"]["start"] = json!({"line": 0});
        assert_eq!(doc_err(vec![c]), "payload changes[0].range.start must contain line and character");
        let mut c = change("insert", Some("x"));
        c["range"]["end"] = json!({"line": -1, "character": 0});
        assert_eq!(doc_err(vec![c]), "payload changes[0].range.end coordinates must be non-negative integers");
        let mut c = change("insert", Some("x"));
        c["range"]["end"] = json!({"line": 1.0, "character": 0});
        assert_eq!(doc_err(vec![c]), "payload changes[0].range.end coordinates must be non-negative integers");
        let mut c = change("insert", Some("x"));
        c["range"]["end"] = json!({"line": true, "character": 0});
        assert_eq!(doc_err(vec![c]), "payload changes[0].range.end coordinates must be non-negative integers");
        let mut c = change("insert", Some("x"));
        c["removed_characters"] = json!(-1);
        assert_eq!(doc_err(vec![c]), "payload changes[0].removed_characters must be a non-negative integer");
        assert_eq!(doc_err(vec![change("insert", None)]), "payload changes[0].text is required for insert and replace");
        assert_eq!(
            doc_err(vec![change("replace", None)]),
            "payload changes[0].text is required for insert and replace"
        );
        let mut c = change("delete", None);
        c["text"] = json!(5);
        assert_eq!(doc_err(vec![c]), "payload changes[0].text must be a string");
        let mut c = change("insert", Some("x"));
        c["text_length"] = json!("1");
        assert_eq!(doc_err(vec![c]), "payload changes[0].text_length must be a non-negative integer");
        let mut c = change("insert", Some("x"));
        c["redacted"] = json!(1);
        assert_eq!(doc_err(vec![c]), "payload changes[0].redacted must be a boolean");
    }

    #[test]
    fn document_change_text_byte_limit() {
        let half = "é".repeat(2048); // 4096 bytes
        ok("vscode", "document_change", doc(vec![change("insert", Some(&half)), change("insert", Some(&half))]));
        assert_eq!(
            doc_err(vec![change("insert", Some(&half)), change("insert", Some(&half)), change("insert", Some("x"))]),
            "payload changes text must be at most 8192 bytes"
        );
    }

    // ---- user_action ------------------------------------------------------

    fn action(action: &str) -> Value {
        json!({
            "url": "https://example.com/x",
            "tab_id": 1,
            "window_id": 2,
            "action": action,
            "target": {"tag": "button", "role": "button", "label": "Save"},
            "sensitive_page": false,
        })
    }

    fn ua_err(payload: Value) -> String {
        err("firefox", "user_action", payload)
    }

    #[test]
    fn user_action_valid() {
        for name in USER_ACTIONS.iter().filter(|name| **name != "scroll") {
            ok("firefox", "user_action", action(name));
        }
        let mut p = action("click");
        p["target"] = json!({"tag": "a", "role": "", "href": "https://x/", "input_type": "submit", "checked": true});
        p["context"] = json!({"kind": "post", "author": "me", "text_excerpt": "hello"});
        p["blocked"] = json!(false);
        ok("firefox", "user_action", p);
    }

    #[test]
    fn user_action_top_level() {
        let mut p = action("click");
        p["extra"] = json!(1);
        assert_eq!(ua_err(p), "user_action payload contains unsupported fields");
        let mut p = action("click");
        p["url"] = json!("");
        assert_eq!(ua_err(p), "payload url must be a non-empty string");
        let mut p = action("click");
        p["url"] = json!("h".repeat(4097));
        assert_eq!(ua_err(p), "payload url must be at most 4096 characters");
        for bad in [json!(-1), json!(true), json!("1"), json!(1.5)] {
            let mut p = action("click");
            p["tab_id"] = bad.clone();
            assert_eq!(ua_err(p), "payload tab_id must be a non-negative integer");
            let mut p = action("click");
            p["window_id"] = bad;
            assert_eq!(ua_err(p), "payload window_id must be a non-negative integer");
        }
        assert_eq!(ua_err(action("hover")), "payload action is invalid");
        let mut p = action("click");
        p["action"] = json!(["click"]);
        assert_eq!(ua_err(p), "payload action is invalid");
        let mut p = action("click");
        p["sensitive_page"] = json!(0);
        assert_eq!(ua_err(p), "payload sensitive_page must be a boolean");
        let mut p = action("click");
        p["blocked"] = json!("no");
        assert_eq!(ua_err(p), "payload blocked must be a boolean");
    }

    #[test]
    fn user_action_target() {
        let mut p = action("click");
        p["target"] = json!({"tag": "a"});
        assert_eq!(ua_err(p), "payload target must contain tag and role");
        let mut p = action("click");
        p["target"] = json!("a");
        assert_eq!(ua_err(p), "payload target must contain tag and role");
        let mut p = action("click");
        p["target"] = json!({"tag": "a", "role": "", "id": "x"});
        assert_eq!(ua_err(p), "payload target contains unsupported fields");
        let mut p = action("click");
        p["target"] = json!({"tag": "", "role": ""});
        assert_eq!(ua_err(p), "payload target.tag must be a non-empty string");
        let mut p = action("click");
        p["target"] = json!({"tag": "t".repeat(65), "role": ""});
        assert_eq!(ua_err(p), "payload target.tag must be at most 64 characters");
        let mut p = action("click");
        p["target"] = json!({"tag": "a", "role": "r".repeat(65)});
        assert_eq!(ua_err(p), "payload target.role must be a string of at most 64 characters");
        let mut p = action("click");
        p["target"] = json!({"tag": "a", "role": null});
        assert_eq!(ua_err(p), "payload target.role must be a string of at most 64 characters");
        for (field, max) in [("label", 160), ("input_type", 64), ("href", 4096)] {
            let mut p = action("click");
            p["target"][field] = json!("x".repeat(max + 1));
            assert_eq!(ua_err(p), format!("payload target.{field} must be a string of at most {max} characters"));
            let mut p = action("click");
            p["target"][field] = json!(1);
            assert_eq!(ua_err(p), format!("payload target.{field} must be a string of at most {max} characters"));
        }
        let mut p = action("click");
        p["target"]["checked"] = json!("yes");
        assert_eq!(ua_err(p), "payload target.checked must be a boolean");
    }

    #[test]
    fn user_action_sensitive_page() {
        let mut p = action("click");
        p["sensitive_page"] = json!(true);
        p["target"] = json!({"tag": "input", "role": "textbox"});
        ok("firefox", "user_action", p.clone());
        p["action"] = json!("form_submit");
        ok("firefox", "user_action", p.clone());
        p["action"] = json!("toggle");
        assert_eq!(ua_err(p.clone()), "sensitive-page actions must be click or form_submit");
        p["action"] = json!("click");
        p["target"]["label"] = json!("Password");
        assert_eq!(ua_err(p.clone()), "sensitive-page targets may contain only tag and role");
        p["target"] = json!({"tag": "input", "role": "textbox"});
        p["context"] = json!({"kind": "k", "text_excerpt": "t"});
        assert_eq!(ua_err(p.clone()), "sensitive-page payload may not include context");
        p.as_object_mut().unwrap().remove("context");
        p["scroll"] = json!({"direction": "up", "position_bucket": 1});
        assert_eq!(ua_err(p), "payload scroll is valid only for scroll actions");
    }

    fn scroll() -> Value {
        let mut p = action("scroll");
        p["target"] = json!({"tag": "document", "role": "document"});
        p["scroll"] = json!({"direction": "down", "position_bucket": 10});
        p
    }

    #[test]
    fn user_action_scroll() {
        ok("firefox", "user_action", scroll());
        let mut p = scroll();
        p["target"]["label"] = json!("x");
        assert_eq!(ua_err(p), "scroll target must be the document");
        let mut p = scroll();
        p["target"]["tag"] = json!("div");
        assert_eq!(ua_err(p), "scroll target must be the document");
        let mut p = scroll();
        p.as_object_mut().unwrap().remove("scroll");
        assert_eq!(ua_err(p), "payload scroll must contain direction and position_bucket");
        let mut p = scroll();
        p["scroll"]["extra"] = json!(1);
        assert_eq!(ua_err(p), "payload scroll must contain direction and position_bucket");
        let mut p = scroll();
        p["scroll"]["direction"] = json!("left");
        assert_eq!(ua_err(p), "payload scroll.direction is invalid");
        for bad in [json!(11), json!(-1), json!(1.0), json!(true)] {
            let mut p = scroll();
            p["scroll"]["position_bucket"] = bad;
            assert_eq!(ua_err(p), "payload scroll.position_bucket must be an integer from 0 to 10");
        }
        let mut p = scroll();
        p["scroll"]["position_bucket"] = json!(0);
        ok("firefox", "user_action", p);
        let mut p = scroll();
        p["context"] = json!({"kind": "k", "text_excerpt": "t"});
        assert_eq!(ua_err(p), "scroll payload may not include context");
        let mut p = action("click");
        p["scroll"] = json!({"direction": "down", "position_bucket": 1});
        assert_eq!(ua_err(p), "payload scroll is valid only for scroll actions");
    }

    #[test]
    fn user_action_context() {
        let with = |context: Value| {
            let mut p = action("reply");
            p["context"] = context;
            p
        };
        assert_eq!(ua_err(with(json!("x"))), "payload context has invalid fields");
        assert_eq!(
            ua_err(with(json!({"kind": "k", "text_excerpt": "t", "x": 1}))),
            "payload context has invalid fields"
        );
        assert_eq!(ua_err(with(json!({"text_excerpt": "t"}))), "payload context.kind must be a non-empty string");
        assert_eq!(
            ua_err(with(json!({"kind": "k".repeat(33), "text_excerpt": "t"}))),
            "payload context.kind must be at most 32 characters"
        );
        assert_eq!(
            ua_err(with(json!({"kind": "k", "author": "", "text_excerpt": "t"}))),
            "payload context.author must be a non-empty string"
        );
        assert_eq!(
            ua_err(with(json!({"kind": "k", "author": "a".repeat(161), "text_excerpt": "t"}))),
            "payload context.author must be at most 160 characters"
        );
        assert_eq!(ua_err(with(json!({"kind": "k"}))), "payload context.text_excerpt must be a non-empty string");
        assert_eq!(
            ua_err(with(json!({"kind": "k", "text_excerpt": "t".repeat(1001)}))),
            "payload context.text_excerpt must be at most 1000 characters"
        );
        ok("firefox", "user_action", with(json!({"kind": "k", "text_excerpt": "t".repeat(1000)})));
    }

    // ---- file_content -----------------------------------------------------

    fn content() -> Value {
        json!({
            "path": "/w/a.md",
            "workspace": "/w",
            "kind": "text",
            "mime": "text/markdown",
            "size_bytes": 10,
            "sha256": "a".repeat(64),
            "excerpt": "hello",
        })
    }

    #[test]
    fn file_content_rules() {
        ok("filesystem", "file_content", content());
        for kind in FILE_CONTENT_KINDS {
            let mut p = content();
            p["kind"] = json!(kind);
            ok("filesystem", "file_content", p);
        }
        let mut p = content();
        p["extra"] = json!(1);
        assert_eq!(err("filesystem", "file_content", p), "file_content payload contains unsupported fields");
        for (field, max) in
            [("path", 4096), ("workspace", 4096), ("kind", 16), ("mime", 128), ("sha256", 64), ("excerpt", 4000)]
        {
            let mut p = content();
            p[field] = json!("x".repeat(max + 1));
            assert_eq!(
                err("filesystem", "file_content", p),
                format!("payload {field} must be at most {max} characters")
            );
            let mut p = content();
            p[field] = json!("");
            assert_eq!(err("filesystem", "file_content", p), format!("payload {field} must be a non-empty string"));
        }
        let mut p = content();
        p["kind"] = json!("video");
        assert_eq!(err("filesystem", "file_content", p), "file_content payload kind is invalid");
        for bad in [json!(-1), json!("10"), json!(false)] {
            let mut p = content();
            p["size_bytes"] = bad;
            assert_eq!(
                err("filesystem", "file_content", p),
                "file_content payload size_bytes must be a non-negative integer"
            );
        }
    }
}
