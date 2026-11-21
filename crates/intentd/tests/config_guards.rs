//! A bad consent file or a workspace that is too broad must not stop or widen capture.

mod common;

use common::{TestDaemon, canonical, paths_for, uuid};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

fn shell(ts: i64) -> Value {
    json!({"id": uuid(), "ts": ts, "source": "shell", "type": "command",
           "payload": {"cmd": "ls", "cwd": "/tmp", "exit_code": 0}})
}

fn tab(ts: i64, url: &str) -> Value {
    json!({"id": uuid(), "ts": ts, "source": "firefox", "type": "tab_change",
           "payload": {"url": url, "title": "Accounts", "tab_id": 1, "window_id": 1}})
}

fn warnings(status: &Value) -> Vec<String> {
    status["config_warnings"]
        .as_array()
        .map(|a| a.iter().filter_map(|w| w.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn a_typo_in_the_blocklist_does_not_stop_capture() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths_for(dir.path());
    std::fs::create_dir_all(paths.config_dir()).unwrap();
    std::fs::write(
        paths.blocked_domains_config(),
        "blocked_domains:\n  - https://chase.com\n  - bank.example:443\n  - not a host\n",
    )
    .unwrap();
    let d = TestDaemon::start(dir.path()).await;
    let now = intent_store::now_ts();

    assert_eq!(d.post_event(&shell(now)).await.0, StatusCode::CREATED);
    let (s, body) = d.post_event(&tab(now, "https://www.chase.com/acct")).await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(body["event"]["payload"]["url"], "[blocked]");
    let (_, body) = d.post_event(&tab(now, "https://bank.example/")).await;
    assert_eq!(body["event"]["payload"]["url"], "[blocked]");
    let (_, body) = d.post_event(&tab(now, "https://example.com/")).await;
    assert_eq!(body["event"]["payload"]["url"], "https://example.com/");

    let (s, status) = d.call(Method::GET, "/v1/status", None).await;
    assert_eq!(s, StatusCode::OK);
    let warnings = warnings(&status);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("not a host"), "{warnings:?}");
    d.stop().await;
}

#[tokio::test]
async fn an_unreadable_blocklist_blocks_every_url_but_keeps_other_capture() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths_for(dir.path());
    std::fs::create_dir_all(paths.config_dir()).unwrap();
    std::fs::write(paths.blocked_domains_config(), "nonsense: true\n").unwrap();
    let d = TestDaemon::start(dir.path()).await;
    let now = intent_store::now_ts();

    assert_eq!(d.post_event(&shell(now)).await.0, StatusCode::CREATED);
    let (s, body) = d.post_event(&tab(now, "https://example.com/")).await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(body["event"]["payload"]["url"], "[blocked]");
    let (_, status) = d.call(Method::GET, "/v1/status", None).await;
    let warnings = warnings(&status);
    assert!(warnings.iter().any(|w| w.contains("blocked-domains")), "{warnings:?}");
    d.stop().await;
}

#[tokio::test]
async fn workspaces_that_are_too_broad_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths_for(dir.path());
    std::fs::create_dir_all(paths.home().join("src")).unwrap();
    let home = canonical(paths.home());
    let d = TestDaemon::start(dir.path()).await;

    for bad in [
        "/".to_string(),
        home.clone(),
        format!("{home}/"),
        paths.data_dir().to_string_lossy().into_owned(),
        canonical(dir.path()),
    ] {
        let (s, body) = d.call(Method::PUT, "/v1/workspaces", Some(json!({"workspaces": [bad]}))).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{bad}: {body}");
        assert!(body["detail"].as_str().is_some_and(|m| !m.is_empty()), "{body}");
    }
    let (_, body) = d.call(Method::GET, "/v1/workspaces", None).await;
    assert_eq!(body["workspaces"], json!([]));

    let project = format!("{home}/src");
    let (s, _) = d.call(Method::PUT, "/v1/workspaces", Some(json!({"workspaces": [project]}))).await;
    assert_eq!(s, StatusCode::OK);
    d.stop().await;
}

#[tokio::test]
async fn a_hand_written_home_workspace_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths_for(dir.path());
    std::fs::create_dir_all(paths.home().join("src")).unwrap();
    std::fs::create_dir_all(paths.config_dir()).unwrap();
    let home = canonical(paths.home());
    let project = format!("{home}/src");
    std::fs::write(paths.workspaces_config(), json!({"workspaces": ["/", home, project]}).to_string()).unwrap();
    std::fs::write(paths.detailed_capture_config(), json!({"editor": {"enabled": true}}).to_string()).unwrap();
    let d = TestDaemon::start(dir.path()).await;

    let watcher = d.state.fs_watch.borrow().clone();
    assert_eq!(watcher.workspaces, vec![std::path::PathBuf::from(&project)]);
    let (_, body) = d.call(Method::GET, "/v1/workspaces", None).await;
    assert_eq!(body["workspaces"], json!([project]));
    let (_, cfg) = d.call_open(Method::GET, "/v1/detailed-capture/config", None).await;
    assert_eq!(cfg["approved_workspaces"], json!([project]));

    let change = |path: &str, workspace: &str| {
        json!({"id": uuid(), "ts": intent_store::now_ts(), "source": "vscode", "type": "document_change",
               "payload": {"path": path, "workspace": workspace, "language": "json",
                           "changes": [{"kind": "insert", "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}},
                                        "removed_characters": 0, "text_length": 1, "text": "x"}]}})
    };
    let (s, _) = d.post_event(&change(&format!("{home}/.docker/config.json"), &home)).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = d.post_event(&change("/etc/hosts", "/")).await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    let (_, status) = d.call(Method::GET, "/v1/status", None).await;
    assert_eq!(warnings(&status).len(), 2, "{status}");
    d.stop().await;
}
