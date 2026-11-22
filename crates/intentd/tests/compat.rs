//! Companion compatibility: the exact JSON the Firefox extension
//! (`capture/integrations/firefox-extension/background.js`), the VS Code extension
//! (`capture/integrations/vscode-extension/events.js`, `runtime.js`) and the shell
//! hook (`intentctl send-shell`) post must be accepted unchanged.

mod common;

use common::{TestDaemon, canonical, uuid};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

fn now() -> i64 {
    intent_store::now_ts()
}

/// `makeEvent(tab)` in background.js.
fn firefox_tab_change() -> Value {
    json!({"id": uuid(), "ts": now(), "source": "firefox", "type": "tab_change",
           "payload": {"url": "https://docs.rs/axum/latest/axum/", "title": "axum - Rust", "tab_id": 12, "window_id": 3}})
}

/// `tabs.onRemoved` handler in background.js.
fn firefox_tab_close() -> Value {
    json!({"id": uuid(), "ts": now(), "source": "firefox", "type": "tab_close",
           "payload": {"url": "https://github.com/Pranav5255/Intent/pulls", "tab_id": 12, "window_id": 3}})
}

/// `makeUserActionEvent(tab, payload)` in background.js (click with post context).
fn firefox_user_action_click() -> Value {
    json!({"id": uuid(), "ts": now(), "source": "firefox", "type": "user_action",
           "payload": {"url": "https://news.ycombinator.com/item", "tab_id": 12, "window_id": 3, "action": "click",
                       "target": {"tag": "a", "role": "link", "label": "Show HN: Intent", "href": "https://example.com/intent"},
                       "sensitive_page": false,
                       "context": {"kind": "post", "text_excerpt": "Show HN: a local-first activity memory", "author": "pg"}}})
}

/// Scroll user action (document target, bucketed position).
fn firefox_user_action_scroll() -> Value {
    json!({"id": uuid(), "ts": now(), "source": "firefox", "type": "user_action",
           "payload": {"url": "https://doc.rust-lang.org/book/", "tab_id": 4, "window_id": 1, "action": "scroll",
                       "target": {"tag": "document", "role": "document"}, "sensitive_page": false,
                       "scroll": {"direction": "down", "position_bucket": 3}}})
}

/// Sensitive page: label/href/context stripped by the extension.
fn firefox_user_action_sensitive() -> Value {
    json!({"id": uuid(), "ts": now(), "source": "firefox", "type": "user_action",
           "payload": {"url": "https://bank.example/login", "tab_id": 5, "window_id": 1, "action": "form_submit",
                       "target": {"tag": "form", "role": ""}, "sensitive_page": true}})
}

/// `event(type, payload)` in events.js (with `schema_version`).
fn vscode(kind: &str, payload: Value) -> Value {
    json!({"id": uuid(), "schema_version": 1, "ts": now(), "source": "vscode", "type": kind, "payload": payload})
}

#[tokio::test]
async fn firefox_and_vscode_payloads_are_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let ws_dir = dir.path().join("work/intent");
    std::fs::create_dir_all(ws_dir.join("src")).unwrap();
    let ws = canonical(&ws_dir);
    let file = format!("{ws}/src/main.rs");
    let d = TestDaemon::start(dir.path()).await;

    // The companions read this open route first.
    let (s, cfg) = d.call_open(Method::GET, "/v1/detailed-capture/config", None).await;
    assert_eq!(s, StatusCode::OK);
    for key in ["editor", "browser", "approved_workspaces"] {
        assert!(cfg.get(key).is_some(), "config.{key} is read by the companions: {cfg}");
    }

    // Metadata events need no consent.
    for event in [
        firefox_tab_change(),
        firefox_tab_close(),
        vscode("workspace_open", json!({"folder": ws})),
        vscode("file_open", json!({"path": file})),
        vscode("file_save", json!({"path": file})),
        vscode("file_edit", json!({"path": file})),
    ] {
        let (s, body) = d.post_event(&event).await;
        assert_eq!(s, StatusCode::CREATED, "{event} -> {body}");
    }

    // Detailed events are dropped (204) until the user consents...
    let doc_change = || {
        vscode(
            "document_change",
            json!({"path": file, "workspace": ws, "language": "rust",
                   "changes": [
                       {"kind": "insert", "range": {"start": {"line": 3, "character": 0}, "end": {"line": 3, "character": 0}},
                        "removed_characters": 0, "text_length": 22, "text": "// TODO: wire intentd\n"},
                       {"kind": "delete", "range": {"start": {"line": 9, "character": 4}, "end": {"line": 9, "character": 10}},
                        "removed_characters": 6},
                       {"kind": "replace", "range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 3}},
                        "removed_characters": 3, "text_length": 3, "text": "pub"}]}),
        )
    };
    assert_eq!(d.post_event(&doc_change()).await.0, StatusCode::NO_CONTENT);
    assert_eq!(d.post_event(&firefox_user_action_click()).await.0, StatusCode::NO_CONTENT);

    // ...then accepted once consent and the workspace are configured.
    let (s, _) = d
        .call(
            Method::PUT,
            "/v1/detailed-capture/config",
            Some(json!({"editor": {"enabled": true}, "browser": {"enabled": true, "context_enabled": true}})),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = d.call(Method::PUT, "/v1/workspaces", Some(json!({"workspaces": [ws]}))).await;
    assert_eq!(s, StatusCode::OK);
    let (_, cfg) = d.call_open(Method::GET, "/v1/detailed-capture/config", None).await;
    assert_eq!(cfg["editor"]["enabled"], true);
    assert_eq!(cfg["approved_workspaces"], json!([ws]));

    for event in
        [doc_change(), firefox_user_action_click(), firefox_user_action_scroll(), firefox_user_action_sensitive()]
    {
        let (s, body) = d.post_event(&event).await;
        assert_eq!(s, StatusCode::CREATED, "{event} -> {body}");
    }

    // A file outside the approved workspace is not captured in detail.
    let outside = vscode(
        "document_change",
        json!({"path": "/etc/hosts", "workspace": ws, "language": "plaintext",
               "changes": [{"kind": "insert", "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}},
                            "removed_characters": 0, "text_length": 1, "text": "x"}]}),
    );
    assert_eq!(d.post_event(&outside).await.0, StatusCode::NO_CONTENT);

    // Shell hook (`intentctl send-shell`) shape, including a redacted secret.
    let shell = json!({"id": uuid(), "ts": now(), "source": "shell", "type": "command",
                       "payload": {"cmd": "curl -H 'Authorization: Bearer abc123' https://x", "cwd": ws, "exit_code": 0, "duration_ms": 420}});
    let (s, body) = d.post_event(&shell).await;
    assert_eq!(s, StatusCode::CREATED, "{body}");
    assert!(!body["event"]["payload"]["cmd"].as_str().unwrap().contains("abc123"), "{body}");

    // The Chrome companion sends the same shapes with `source: "chrome"`.
    for mut event in [firefox_tab_change(), firefox_tab_close(), firefox_user_action_click()] {
        event["source"] = json!("chrome");
        event["id"] = json!(uuid());
        let (s, body) = d.post_event(&event).await;
        assert_eq!(s, StatusCode::CREATED, "{event} -> {body}");
    }
    let (_, status) = d.call(Method::GET, "/v1/status", None).await;
    assert_eq!(status["sources"]["chrome"]["event_count"], 3, "{status}");

    // The VS Code runtime retries with the same id: idempotent.
    let retry = vscode("file_save", json!({"path": file}));
    assert_eq!(d.post_event(&retry).await.0, StatusCode::CREATED);
    assert_eq!(d.post_event(&retry).await.0, StatusCode::OK);
    d.stop().await;
}
