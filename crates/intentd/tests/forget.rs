//! Forget a day, forget a project, retention: the data must not come back.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{TestDaemon, local_ts, uuid};
use intent_types::PipelineResult;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

fn shell(ts: i64, cmd: &str, cwd: &str) -> Value {
    json!({"id": uuid(), "ts": ts, "source": "shell", "type": "command",
           "payload": {"cmd": cmd, "cwd": cwd, "exit_code": 0}})
}

fn tab(ts: i64, url: &str, title: &str) -> Value {
    json!({"id": uuid(), "ts": ts, "source": "firefox", "type": "tab_change",
           "payload": {"url": url, "title": title, "tab_id": 1, "window_id": 1}})
}

async fn post_all(d: &TestDaemon, events: &[Value]) {
    for e in events {
        let (s, body) = d.post_event(e).await;
        assert_eq!(s, StatusCode::CREATED, "{body}");
    }
}

async fn run_day(d: &TestDaemon, date: &str) -> Value {
    let (s, result) = d.call(Method::POST, &format!("/pipeline/run?date={date}"), None).await;
    assert_eq!(s, StatusCode::OK, "{result}");
    result
}

async fn intents_text(d: &TestDaemon, date: &str) -> String {
    d.call(Method::GET, &format!("/intents?date={date}"), None).await.1.to_string()
}

#[tokio::test]
async fn forget_date_deletes_raw_events_and_the_day_stays_forgotten() {
    let dir = tempfile::tempdir().unwrap();
    let d = TestDaemon::start(dir.path()).await;
    let (date, t0) = local_ts(1, 10, 0);
    post_all(
        &d,
        &[
            tab(t0, "https://secret-bank.example/", "secret bank"),
            tab(t0 + 60, "https://secret-bank.example/2", "secret bank 2"),
        ],
    )
    .await;
    run_day(&d, &date).await;
    assert!(intents_text(&d, &date).await.contains("secret"));
    let key = format!("scheduler_day_hash:{date}");
    d.state.store.set_metadata(&key, "rev:2|count:2|last:x").unwrap();

    let (s, report) = d.call(Method::DELETE, &format!("/v1/memory/date/{date}"), None).await;
    assert_eq!(s, StatusCode::OK, "{report}");
    assert_eq!(report["deleted_event_count"], 2, "{report}");
    let (_, events) = d.call(Method::GET, &format!("/v1/events?date={date}"), None).await;
    assert_eq!(events, json!([]), "the raw events are gone");
    assert_eq!(d.state.store.get_metadata(&key).unwrap(), None, "the scheduler forgets the day too");

    // The user keeps working on the same day.
    post_all(&d, &[tab(t0 + 120, "https://example.org/", "unrelated page")]).await;
    run_day(&d, &date).await;
    let text = intents_text(&d, &date).await;
    assert!(!text.contains("secret"), "the forgotten activity came back: {text}");
    d.stop().await;
}

#[tokio::test]
async fn forget_date_waits_for_a_running_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let d = Arc::new(TestDaemon::start(dir.path()).await);
    let (date, t0) = local_ts(1, 10, 0);
    post_all(&d, &[tab(t0, "https://race.example/", "race page")]).await;
    let result: PipelineResult = serde_json::from_value(run_day(&d, &date).await).unwrap();

    // A run holds the lock; the forget arrives while it works and must wait for it.
    let guard = d.state.pipeline_lock.lock().await;
    let forget = {
        let d = Arc::clone(&d);
        let date = date.clone();
        tokio::spawn(async move { d.call(Method::DELETE, &format!("/v1/memory/date/{date}"), None).await.0 })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    // The run finishes after the forget started and saves its intents.
    d.state.store.save_pipeline_run(&date, &result).unwrap();
    drop(guard);
    assert_eq!(forget.await.unwrap(), StatusCode::OK);
    let (_, intents) = d.call(Method::GET, &format!("/intents?date={date}"), None).await;
    assert_eq!(intents, json!([]), "a run that overlapped the forget saved intents after it");
}

#[tokio::test]
async fn forget_project_deletes_its_events_and_removes_it_from_mixed_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let d = TestDaemon::start(dir.path()).await;
    let (date, t0) = local_ts(1, 9, 0);
    let mut events = Vec::new();
    // One session that mixes two projects.
    for i in 0..4 {
        events.push(shell(t0 + i * 30, "cargo build", "/home/dev/src/alphaproj"));
    }
    for i in 0..4 {
        events.push(shell(t0 + 150 + i * 30, "make test", "/home/dev/src/zorblax"));
        events.push(json!({"id": uuid(), "ts": t0 + 160 + i * 30, "source": "vscode", "type": "file_save",
                           "payload": {"path": "/home/dev/src/zorblax/main.rs"}}));
    }
    post_all(&d, &events).await;
    run_day(&d, &date).await;
    assert!(intents_text(&d, &date).await.contains("project:zorblax"));

    let (s, report) = d.call(Method::DELETE, "/v1/memory/project/zorblax", None).await;
    assert_eq!(s, StatusCode::OK, "{report}");
    assert_eq!(report["deleted_event_count"], 8, "{report}");

    let text = intents_text(&d, &date).await;
    assert!(!text.contains("zorblax"), "the project is still in the day's intents: {text}");
    assert!(text.contains("project:alphaproj"), "the other project is rebuilt: {text}");
    let (_, hits) = d.call(Method::GET, "/intents/search?q=zorblax", None).await;
    assert!(!hits.to_string().contains("zorblax"), "{hits}");
    let (_, raw) = d.call(Method::GET, &format!("/v1/events?date={date}"), None).await;
    assert!(!raw.to_string().contains("zorblax"), "{raw}");
    assert_eq!(raw.as_array().unwrap().len(), 4);

    // The next run does not bring it back.
    post_all(&d, &[tab(t0 + 600, "https://example.org/", "unrelated page")]).await;
    run_day(&d, &date).await;
    assert!(!intents_text(&d, &date).await.contains("zorblax"));
    d.stop().await;
}

#[tokio::test]
async fn retention_purge_removes_old_intents_and_reports_them() {
    let dir = tempfile::tempdir().unwrap();
    let d = TestDaemon::start(dir.path()).await;
    let (date, t0) = local_ts(40, 10, 0);
    post_all(
        &d,
        &[shell(t0, "cargo test", "/home/dev/src/oldproj"), shell(t0 + 30, "cargo run", "/home/dev/src/oldproj")],
    )
    .await;
    run_day(&d, &date).await;
    assert!(intents_text(&d, &date).await.contains("oldproj"));

    let (_, preview) = d.call(Method::GET, "/v1/retention/preview?metadata_days=30", None).await;
    assert!(preview["eligible"]["intents"].as_i64().unwrap() > 0, "{preview}");
    let (s, body) =
        d.call(Method::POST, "/v1/retention/purge", Some(json!({"metadata_days": 30, "confirm": true}))).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert!(body["deleted"]["intents"].as_i64().unwrap() > 0, "{body}");
    assert_eq!(d.call(Method::GET, &format!("/intents?date={date}"), None).await.1, json!([]));
    let (_, hits) = d.call(Method::GET, "/intents/search?q=oldproj", None).await;
    assert_eq!(hits, json!([]));
    d.stop().await;
}
