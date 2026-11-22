//! End-to-end API tests against a real router on an ephemeral port.

mod common;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use common::{TestDaemon, canonical, local_ts, test_options, uuid};
use intent_llm::client::{ToolCall, ToolTurn};
use intent_llm::mock::ScriptedClient;
use intent_llm::{Harness, HashingEmbedder, Providers};
use intentd::WriterConfig;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

fn shell(ts: i64, cmd: &str, cwd: &str, exit: i64) -> Value {
    json!({"id": uuid(), "ts": ts, "source": "shell", "type": "command",
           "payload": {"cmd": cmd, "cwd": cwd, "exit_code": exit}})
}

#[tokio::test]
async fn ingest_status_codes_and_dedupe() {
    let dir = tempfile::tempdir().unwrap();
    let d = TestDaemon::start(dir.path()).await;
    let now = intent_store::now_ts();

    let ev = shell(now, "ls -la", "/tmp", 0);
    let (s, body) = d.post_event(&ev).await;
    assert_eq!(s, StatusCode::CREATED);
    assert_eq!(body["ok"], true);
    assert_eq!(body["inserted"], true);
    assert_eq!(body["event"]["source"], "shell");
    assert!(body["event"]["ingested_at"].is_i64());

    // Same id again: idempotent, 200 with the stored copy.
    let (s, body) = d.post_event(&ev).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["inserted"], false);

    // Unprefixed 1.x alias.
    let (s, _) = d.call_open(Method::POST, "/event", Some(shell(now, "pwd", "/tmp", 0))).await;
    assert_eq!(s, StatusCode::CREATED);

    // Detailed capture is off by default → silently dropped.
    let detailed = json!({"id": uuid(), "ts": now, "source": "vscode", "type": "document_change",
        "payload": {"path": "/w/a.py", "workspace": "/w", "language": "python",
                    "changes": [{"kind": "insert", "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}},
                                 "removed_characters": 0, "text_length": 1, "text": "x"}]}});
    let (s, body) = d.post_event(&detailed).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(body, Value::Null);

    // Invalid: missing required payload field, bad id, unknown kind, malformed JSON.
    let (s, body) = d
        .post_event(&json!({"id": uuid(), "ts": now, "source": "shell", "type": "command", "payload": {"cmd": "x"}}))
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body["detail"].as_str().unwrap().contains("cwd"), "{body}");
    let (s, _) =
        d.post_event(&json!({"id": "nope", "ts": now, "source": "shell", "type": "command", "payload": {}})).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _) =
        d.post_event(&json!({"id": uuid(), "ts": now, "source": "shell", "type": "teleport", "payload": {}})).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let resp = d
        .client
        .post(format!("{}/v1/event", d.base))
        .header("content-type", "application/json")
        .body("{")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let (_, status) = d.call(Method::GET, "/v1/status", None).await;
    let ing = &status["ingestion"];
    assert_eq!(ing["inserted"], 2);
    assert_eq!(ing["duplicates"], 1);
    assert_eq!(ing["dropped_by_reason"]["detailed_capture_disabled"], 1);
    assert_eq!(status["sources"]["shell"]["event_count"], 2);
    assert_eq!(status["sources"]["shell"]["healthy"], true);
    d.stop().await;
}

#[tokio::test]
async fn queue_full_is_503() {
    let dir = tempfile::tempdir().unwrap();
    let (mut opts, launched) = test_options(dir.path());
    opts.writer = WriterConfig {
        queue_capacity: 1,
        max_batch: 1,
        batch_wait: Duration::ZERO,
        submit_timeout: Duration::from_secs(10),
        write_delay: Duration::from_millis(800),
    };
    let d = Arc::new(TestDaemon::start_with(opts, launched).await);
    let now = intent_store::now_ts();
    // #1 is taken by the (slow) writer, #2 waits in the queue, #3 finds it full.
    let first = {
        let d = Arc::clone(&d);
        tokio::spawn(async move { d.post_event(&shell(now, "a", "/", 0)).await.0 })
    };
    tokio::time::sleep(Duration::from_millis(150)).await;
    let second = {
        let d = Arc::clone(&d);
        tokio::spawn(async move { d.post_event(&shell(now, "b", "/", 0)).await.0 })
    };
    tokio::time::sleep(Duration::from_millis(150)).await;
    let (third, body) = d.post_event(&shell(now, "c", "/", 0)).await;
    assert_eq!(third, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(first.await.unwrap(), StatusCode::CREATED);
    assert_eq!(second.await.unwrap(), StatusCode::CREATED);
    let (_, status) = d.call(Method::GET, "/v1/status", None).await;
    assert_eq!(status["ingestion"]["dropped_by_reason"]["queue_full"], 1);
    assert!(status["ingestion"]["queue_high_watermark"].as_u64().unwrap() >= 1);
}

#[tokio::test]
async fn pause_persists_across_restart_and_expires() {
    let dir = tempfile::tempdir().unwrap();
    let d = TestDaemon::start(dir.path()).await;
    let now = intent_store::now_ts();
    let (s, body) =
        d.call(Method::POST, "/v1/capture/pause", Some(json!({"paused": true, "duration_seconds": 3600}))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["paused"], true);
    let until = body["paused_until"].as_i64().unwrap();
    assert!(until >= now + 3599);
    let (s, _) = d.post_event(&shell(now, "secret stuff", "/", 0)).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    d.stop().await;

    // Restart on the same data: still paused (1.x forgot the pause on restart).
    let d = TestDaemon::start(dir.path()).await;
    let (_, status) = d.call(Method::GET, "/v1/status", None).await;
    assert_eq!(status["capture_paused"], true);
    assert_eq!(status["paused_until"], until);
    let (s, _) = d.post_event(&shell(now, "still paused", "/", 0)).await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    // A short timed pause expires on its own.
    let (_, body) =
        d.call(Method::POST, "/v1/capture/pause", Some(json!({"paused": true, "duration_seconds": 1}))).await;
    assert_eq!(body["paused"], true);
    tokio::time::sleep(Duration::from_millis(1300)).await;
    let (s, _) = d.post_event(&shell(now, "resumed", "/", 0)).await;
    assert_eq!(s, StatusCode::CREATED);

    // Explicit resume.
    d.call(Method::POST, "/v1/capture/pause", Some(json!({"paused": true}))).await;
    let (_, body) = d.call(Method::POST, "/v1/capture/pause", Some(json!({"paused": false}))).await;
    assert_eq!(body["paused"], false);
    assert_eq!(body["paused_until"], Value::Null);
    let (s, _) = d.call(Method::POST, "/v1/capture/pause", Some(json!({"paused": true, "duration_seconds": 0}))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // The Tauri app sends an absolute `until` and reads `capture_paused_until`.
    let until = intent_store::now_ts() + 900;
    let (s, body) = d.call(Method::POST, "/v1/capture/pause", Some(json!({"paused": true, "until": until}))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["until"], until);
    let (_, status) = d.call(Method::GET, "/v1/status", None).await;
    assert_eq!(status["capture_paused_until"], until);
    assert!(status["db_size_bytes"].as_u64().unwrap() > 0);
    d.stop().await;
}

#[tokio::test]
async fn auth_matrix() {
    let dir = tempfile::tempdir().unwrap();
    let d = TestDaemon::start(dir.path()).await;
    let protected: &[(Method, &str)] = &[
        (Method::GET, "/v1/events"),
        (Method::GET, "/events"),
        (Method::GET, "/v1/export/day?date=2026-01-01"),
        (Method::GET, "/v1/export/day/meta?date=2026-01-01"),
        (Method::GET, "/v1/events/incremental?date=2026-01-01"),
        (Method::PUT, "/v1/detailed-capture/config"),
        (Method::GET, "/v1/workspaces"),
        (Method::PUT, "/v1/workspaces"),
        (Method::GET, "/v1/filesystem-capture"),
        (Method::PUT, "/v1/filesystem-capture"),
        (Method::GET, "/v1/retention/policy"),
        (Method::PUT, "/v1/retention/policy"),
        (Method::GET, "/v1/intelligence/preview-sample"),
        (Method::POST, "/v1/detailed-capture/purge"),
        (Method::GET, "/v1/retention/preview?detailed_days=1"),
        (Method::POST, "/v1/retention/purge"),
        (Method::POST, "/v1/restore"),
        (Method::POST, "/restore"),
        (Method::POST, "/v1/capture/pause"),
        (Method::GET, "/v1/status"),
        (Method::GET, "/status"),
        (Method::GET, "/intents/yesterday"),
        (Method::GET, "/intents?date=2026-01-01"),
        (Method::GET, "/intents/search?q=x"),
        (Method::GET, "/intents/stats?date_from=2026-01-01&date_to=2026-01-02"),
        (Method::GET, "/intents/digest"),
        (Method::GET, "/intents/current"),
        (Method::GET, "/intents/prediction"),
        (Method::GET, "/intents/abc"),
        (Method::GET, "/intents/abc/context"),
        (Method::POST, "/resume/select"),
        (Method::POST, "/copilot/query"),
        (Method::GET, "/copilot/briefing/abc"),
        (Method::GET, "/settings/llm"),
        (Method::PUT, "/settings/llm"),
        (Method::POST, "/settings/llm/gemini-credentials"),
        (Method::DELETE, "/settings/llm/gemini-credentials"),
        (Method::POST, "/pipeline/run"),
        (Method::POST, "/pipeline/run-replay"),
        (Method::POST, "/pipeline/recompute?date=2026-01-01"),
        (Method::DELETE, "/v1/memory/date/2026-01-01"),
        (Method::DELETE, "/v1/memory/project/x"),
    ];
    for (method, path) in protected {
        let (s, body) = d.call_open(method.clone(), path, Some(json!({}))).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{method} {path} must need the token");
        assert_eq!(body["detail"], "missing or invalid token");
    }
    // Wrong token.
    let resp = d.client.get(format!("{}/v1/status", d.base)).bearer_auth("0".repeat(64)).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Open companion surface.
    for path in ["/healthz", "/v1/detailed-capture/config", "/v1/config"] {
        let (s, _) = d.call_open(Method::GET, path, None).await;
        assert_eq!(s, StatusCode::OK, "{path} must stay open");
    }
    let (s, cfg) = d.call_open(Method::GET, "/v1/detailed-capture/config", None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(cfg["editor"]["enabled"].is_boolean());
    d.stop().await;
}

#[tokio::test]
async fn cors_only_allows_extension_and_app_origins() {
    let dir = tempfile::tempdir().unwrap();
    let (mut opts, launched) = test_options(dir.path());
    opts.firefox_origin = Some("moz-extension://abc-123".into());
    let d = TestDaemon::start_with(opts, launched).await;
    let preflight = |origin: &'static str| {
        d.client
            .request(Method::OPTIONS, format!("{}/v1/event", d.base))
            .header("Origin", origin)
            .header("Access-Control-Request-Method", "POST")
            .header("Access-Control-Request-Headers", "content-type")
            .send()
    };
    for origin in ["moz-extension://abc-123", "tauri://localhost", "http://tauri.localhost"] {
        let resp = preflight(origin).await.unwrap();
        assert_eq!(resp.headers().get("access-control-allow-origin").unwrap(), origin);
    }
    for origin in ["http://localhost:5173", "https://evil.example"] {
        let resp = preflight(origin).await.unwrap();
        assert!(resp.headers().get("access-control-allow-origin").is_none(), "{origin} must be refused");
    }
    d.stop().await;
}

#[tokio::test]
async fn workspace_put_reconfigures_watcher() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("project");
    std::fs::create_dir_all(&ws).unwrap();
    let ws = canonical(&ws);
    let d = TestDaemon::start(dir.path()).await;
    assert!(d.state.fs_watch.borrow().workspaces.is_empty());

    let (s, body) = d.call(Method::PUT, "/v1/workspaces", Some(json!({"workspaces": [format!("{ws}/")]}))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["workspaces"], json!([ws.clone()]));
    let watcher = d.state.fs_watch.borrow().clone();
    assert_eq!(watcher.workspaces, vec![std::path::PathBuf::from(&ws)]);
    assert!(!watcher.all_accessible);

    let (s, body) = d.call(Method::PUT, "/v1/filesystem-capture", Some(json!({"all_accessible": true}))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body, json!({"all_accessible": true}));
    assert!(d.state.fs_watch.borrow().all_accessible);

    let (s, _) =
        d.call(Method::PUT, "/v1/detailed-capture/config", Some(json!({"filesystem": {"enabled": true}}))).await;
    assert_eq!(s, StatusCode::OK);
    assert!(d.state.fs_watch.borrow().capture_content);

    let (_, body) = d.call(Method::GET, "/v1/workspaces", None).await;
    assert_eq!(body["workspaces"], json!([ws]));
    let (s, _) = d.call(Method::PUT, "/v1/workspaces", Some(json!({"workspaces": [" "]}))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    d.stop().await;
}

#[tokio::test]
async fn export_and_incremental_paging() {
    let dir = tempfile::tempdir().unwrap();
    let d = TestDaemon::start(dir.path()).await;
    let (date, base) = local_ts(1, 11, 0);
    let mut ids = BTreeSet::new();
    for i in 0..7 {
        // Two events share a timestamp to exercise the keyset tie-break.
        let ev = shell(base + (i / 2) * 10, &format!("echo {i}"), "/tmp", 0);
        let (s, body) = d.post_event(&ev).await;
        assert_eq!(s, StatusCode::CREATED);
        ids.insert(body["event"]["id"].as_str().unwrap().to_string());
    }
    let (s, meta) = d.call(Method::GET, &format!("/v1/export/day/meta?date={date}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(meta["event_count"], 7);
    assert_eq!(meta["first_ts"], base);
    let (_, alias) = d.call(Method::GET, &format!("/export/day/meta?date={date}"), None).await;
    assert_eq!(alias["content_hash"], meta["content_hash"]);

    let mut seen = BTreeSet::new();
    let mut cursor: Option<(i64, String)> = None;
    let mut pages = 0;
    loop {
        let mut path = format!("/v1/events/incremental?date={date}&limit=3");
        if let Some((ts, id)) = &cursor {
            path.push_str(&format!("&after_ts={ts}&after_id={id}"));
        }
        let (s, page) = d.call(Method::GET, &path, None).await;
        assert_eq!(s, StatusCode::OK, "{page}");
        assert_eq!(page["page_meta"]["content_hash"], meta["content_hash"]);
        for e in page["events"].as_array().unwrap() {
            assert!(seen.insert(e["id"].as_str().unwrap().to_string()), "duplicate across pages");
        }
        pages += 1;
        if page["has_more"] == false {
            assert_eq!(page["next_cursor"], Value::Null);
            break;
        }
        let c = &page["next_cursor"];
        cursor = Some((c["after_ts"].as_i64().unwrap(), c["after_id"].as_str().unwrap().to_string()));
    }
    assert_eq!(pages, 3);
    assert_eq!(seen, ids);

    let (_, export) = d.call(Method::GET, &format!("/v1/export/day?date={date}"), None).await;
    assert_eq!(export["events"].as_array().unwrap().len(), 7);
    let (_, by_date) = d.call(Method::GET, &format!("/v1/events?date={date}"), None).await;
    assert_eq!(by_date.as_array().unwrap().len(), 7);
    let (s, _) = d.call(Method::GET, &format!("/v1/events?date={date}&since=1"), None).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _) = d.call(Method::GET, &format!("/v1/events/incremental?date={date}&after_ts=1"), None).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _) = d.call(Method::GET, &format!("/v1/events/incremental?date={date}&limit=0"), None).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _) = d.call(Method::GET, "/v1/export/day?date=2026-02-30", None).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    d.stop().await;
}

/// A realistic morning: coding + tests + docs in one project, then unrelated browsing.
async fn seed_day(d: &TestDaemon, project: &str) -> (String, usize) {
    let (date, t0) = local_ts(1, 9, 0);
    let file = format!("{project}/crates/intentd/src/lib.rs");
    let mut events = Vec::new();
    for i in 0..6 {
        let t = t0 + i * 120;
        events.push(json!({"id": uuid(), "ts": t, "source": "linux", "type": "app_focus",
                           "payload": {"app": "Code", "title": "lib.rs - intent - Visual Studio Code"}}));
        events.push(
            json!({"id": uuid(), "ts": t + 5, "source": "vscode", "type": "file_open", "payload": {"path": file}}),
        );
        events.push(
            json!({"id": uuid(), "ts": t + 30, "source": "vscode", "type": "file_save", "payload": {"path": file}}),
        );
        events.push(shell(t + 60, "cargo test -p intentd", project, if i == 2 { 101 } else { 0 }));
        events.push(json!({"id": uuid(), "ts": t + 90, "source": "firefox", "type": "tab_change",
                           "payload": {"url": "https://docs.rs/axum/latest/axum/", "title": "axum - Rust", "tab_id": 3, "window_id": 1}}));
    }
    events.push(shell(t0 + 900, "git commit -m wire daemon", project, 0));
    for e in &events {
        let (s, body) = d.post_event(e).await;
        assert_eq!(s, StatusCode::CREATED, "{body}");
    }
    (date, events.len())
}

#[tokio::test]
async fn pipeline_search_digest_resume_restore() {
    let dir = tempfile::tempdir().unwrap();
    let d = TestDaemon::start(dir.path()).await;
    let project = "/home/dev/src/intent";
    let (date, n) = seed_day(&d, project).await;

    let (s, result) = d.call(Method::POST, &format!("/pipeline/run?date={date}"), None).await;
    assert_eq!(s, StatusCode::OK, "{result}");
    assert_eq!(result["cached"], false);
    let intents = result["intents"].as_array().unwrap();
    assert!(!intents.is_empty());
    let total: i64 = intents.iter().map(|i| i["stats"]["event_count"].as_i64().unwrap()).sum();
    assert_eq!(total as usize, n);

    // Second run on unchanged input hits the cache.
    let (_, again) = d.call(Method::POST, &format!("/pipeline/run?date={date}"), None).await;
    assert_eq!(again["cached"], true);
    assert_eq!(again["intents"].as_array().unwrap().len(), intents.len());

    let (s, stored) = d.call(Method::GET, &format!("/intents?date={date}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(stored.as_array().unwrap().len(), intents.len());
    let (_, yesterday) = d.call(Method::GET, "/intents/yesterday", None).await;
    assert_eq!(yesterday.as_array().unwrap().len(), intents.len());

    let root = &stored[0];
    let id = root["id"].as_str().unwrap();
    let (s, one) = d.call(Method::GET, &format!("/intents/{id}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(one["label"], root["label"]);
    let (_, ctx) = d.call(Method::GET, &format!("/intents/{id}/context"), None).await;
    assert!(ctx["markdown"].as_str().unwrap().starts_with("# "));
    let (s, _) = d.call(Method::GET, "/intents/does-not-exist", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Search by a word of the label.
    let word = root["label"].as_str().unwrap().split_whitespace().find(|w| w.len() > 3).unwrap().to_string();
    let (s, hits) =
        d.call(Method::GET, &format!("/intents/search?q={word}&date_from={date}&date_to={date}"), None).await;
    assert_eq!(s, StatusCode::OK, "{hits}");
    assert!(hits.as_array().unwrap().iter().any(|h| h["date"] == date.as_str()), "{hits}");
    let (s, _) = d.call(Method::GET, "/intents/search?q=%20", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // The overlay scopes search to a project tag.
    if let Some(tag) =
        root["tags"].as_array().unwrap().iter().filter_map(Value::as_str).find_map(|t| t.strip_prefix("project:"))
    {
        let (_, scoped) = d.call(Method::GET, &format!("/intents/search?q={word}&project={tag}"), None).await;
        assert!(!scoped.as_array().unwrap().is_empty(), "{scoped}");
    }
    let (_, other) = d.call(Method::GET, &format!("/intents/search?q={word}&project=no-such-project"), None).await;
    assert!(other.as_array().unwrap().is_empty(), "{other}");

    let (_, digest) = d.call(Method::GET, &format!("/intents/digest?date={date}"), None).await;
    assert_eq!(digest["date"], date.as_str());
    assert_eq!(digest["intent_count"].as_i64().unwrap() as usize, intents.len());
    assert!(!digest["headline"].as_str().unwrap().is_empty());
    let (_, default_digest) = d.call(Method::GET, "/intents/digest", None).await;
    assert_eq!(default_digest["date"], date.as_str(), "digest defaults to yesterday");

    let (s, stats) = d.call(Method::GET, &format!("/intents/stats?date_from={date}&date_to={date}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(stats["event_count"].as_i64().unwrap() as usize, n);

    // Resume selection → restore planning with a recording executor.
    let (s, sel) = d.call(Method::POST, "/resume/select", Some(json!({"intent_id": id}))).await;
    assert_eq!(s, StatusCode::OK, "{sel}");
    assert_eq!(sel["needs_picker"], false);
    let payload = sel["selected"]["resume_payload"].clone();
    assert!(
        !payload["files"].as_array().unwrap().is_empty() || !payload["urls"].as_array().unwrap().is_empty(),
        "{payload}"
    );
    let (s, sel_q) = d.call(Method::POST, "/resume/select", Some(json!({"query": word}))).await;
    assert_eq!(s, StatusCode::OK);
    assert!(!sel_q["candidates"].as_array().unwrap().is_empty());

    let mut restore = payload.clone();
    restore["mode"] = json!("resume");
    let (s, res) = d.call(Method::POST, "/v1/restore", Some(restore)).await;
    assert_eq!(s, StatusCode::OK, "{res}");
    assert_eq!(res["ok"], true, "{res}");
    let launched = d.launched.lock().unwrap().clone();
    assert!(!launched.is_empty(), "the planned commands went to the mock executor");
    assert!(launched.iter().all(|c| c.program.starts_with("/usr/bin")));
    let (s, _) = d.call(Method::POST, "/v1/restore", Some(json!({"urls": ["javascript:alert(1)"]}))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    // Replay an export under its own date.
    let (_, export) = d.call(Method::GET, &format!("/v1/export/day?date={date}"), None).await;
    let (s, replay) = d.call(Method::POST, "/pipeline/run-replay", Some(export)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(!replay["intents"].as_array().unwrap().is_empty());

    // Recompute forces a fresh run; memory deletion forgets the day.
    let (_, recomputed) = d.call(Method::POST, &format!("/pipeline/recompute?date={date}"), None).await;
    assert_eq!(recomputed["cached"], false);
    let (s, del) = d.call(Method::DELETE, &format!("/v1/memory/date/{date}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(del["deleted_count"].as_u64().unwrap() >= intents.len() as u64);
    let (_, after) = d.call(Method::GET, &format!("/intents?date={date}"), None).await;
    assert!(after.as_array().unwrap().is_empty());
    let (s, _) = d.call(Method::DELETE, "/v1/memory/date/2026-13-01", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let (_, status) = d.call(Method::GET, "/v1/status", None).await;
    assert_eq!(status["pipeline"]["last_run"]["date"], date.as_str());
    assert!(status["database"]["size_bytes"].as_u64().unwrap() > 0);
    d.stop().await;
}

#[tokio::test]
async fn retention_preview_and_purge_requires_confirm() {
    let dir = tempfile::tempdir().unwrap();
    let d = TestDaemon::start(dir.path()).await;
    let now = intent_store::now_ts();
    for days in [40, 20, 1] {
        d.post_event(&shell(now - days * 86_400, "old", "/", 0)).await;
    }
    let (s, preview) = d.call(Method::GET, "/v1/retention/preview?metadata_days=30", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(preview["eligible"]["metadata"], 1);
    assert_eq!(preview["metadata_days"], 30);
    let (s, _) = d.call(Method::GET, "/v1/retention/preview", None).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    let (s, body) = d.call(Method::POST, "/v1/retention/purge", Some(json!({"metadata_days": 10}))).await;
    assert_eq!(s, StatusCode::CONFLICT, "{body}");
    let (s, _) = d.call(Method::POST, "/v1/retention/purge", Some(json!({"metadata_days": 0, "confirm": true}))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, body) = d.call(Method::POST, "/retention/purge", Some(json!({"metadata_days": 10, "confirm": true}))).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], true);
    assert_eq!(body["deleted"]["total"], 2);
    let (_, events) = d.call(Method::GET, "/v1/events", None).await;
    assert_eq!(events.as_array().unwrap().len(), 1);

    // Policy round trip; the daily job applies it.
    let (s, policy) =
        d.call(Method::PUT, "/v1/retention/policy", Some(json!({"metadata_days": 2, "detailed_days": null}))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(policy, json!({"metadata_days": 2, "detailed_days": null}));
    let (s, _) = d.call(Method::PUT, "/v1/retention/policy", Some(json!({"metadata_days": 0}))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    d.post_event(&shell(now - 5 * 86_400, "older than policy", "/", 0)).await;
    let report = intentd::jobs::enforce_retention(&d.state).await.unwrap();
    assert_eq!(report.deleted.total, 1);
    let (_, status) = d.call(Method::GET, "/v1/status", None).await;
    assert_eq!(status["retention"]["last_run"]["deleted"]["total"], 1);
    d.stop().await;
}

#[tokio::test]
async fn settings_round_trip_rebuilds_providers() {
    let dir = tempfile::tempdir().unwrap();
    let d = TestDaemon::start(dir.path()).await;
    let (s, summary) = d.call(Method::GET, "/settings/llm", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(summary["provider"], "none");

    let (s, summary) = d
        .call(
            Method::PUT,
            "/settings/llm",
            Some(json!({"provider": "local", "model": "qwen2.5:3b", "base_url": "http://127.0.0.1:9/v1",
                        "embedding_model": "nomic-embed-text"})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{summary}");
    assert_eq!(summary["provider"], "local");
    assert_eq!(summary["model"], "qwen2.5:3b");
    assert_eq!(summary["local_server_reachable"], false, "nothing listens on port 9");
    assert_eq!(d.state.llm().settings.embedding_model.as_deref(), Some("nomic-embed-text"));
    let path = d.state.paths.llm_config();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("qwen2.5:3b"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }

    // Cloud provider needs a key; a cloud http base URL is refused.
    let (s, body) = d
        .call(Method::PUT, "/settings/llm", Some(json!({"provider": "openai", "base_url": "http://example.com/v1"})))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    let (s, summary) =
        d.call(Method::PUT, "/settings/llm", Some(json!({"provider": "groq", "api_key": "gsk_test"}))).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(summary["api_key_configured"], true);
    assert!(summary.get("api_key").is_none(), "secrets never leave the daemon");
    let (s, _) = d.call(Method::PUT, "/settings/llm", Some(json!({"provider": "none", "bogus": 1}))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _) = d.call(Method::POST, "/settings/llm/gemini-credentials", Some(json!({"credentials": ""}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) =
        d.call(Method::POST, "/settings/llm/gemini-credentials", Some(json!({"credentials": "{\"type\": 1}"}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    d.stop().await;

    // Survives restart.
    let d = TestDaemon::start(dir.path()).await;
    let (_, summary) = d.call(Method::GET, "/settings/llm", None).await;
    assert_eq!(summary["provider"], "groq");
    d.stop().await;
}

#[tokio::test]
async fn copilot_503_when_unconfigured_and_answers_with_mock_provider() {
    let dir = tempfile::tempdir().unwrap();
    let d = TestDaemon::start(dir.path()).await;
    let (s, body) = d.call(Method::POST, "/copilot/query", Some(json!({"question": "what did I do?"}))).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["code"], "copilot_not_configured");
    let (s, _) = d.call(Method::GET, "/copilot/briefing/abc", None).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    let (s, _) = d.call(Method::POST, "/copilot/query", Some(json!({"question": "x", "mode": "narrative"}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    d.stop().await;

    // Same data, scripted model: one tool call, then an answer.
    let (mut opts, launched) = test_options(dir.path());
    opts.provider_factory = Arc::new(|_| {
        let client = ScriptedClient::new(false);
        client.push_tools(Ok(ToolTurn {
            text: None,
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "search_intents".into(),
                arguments: json!({"query": "cargo"}),
                signature: None,
            }],
        }));
        client.push_tools(Ok(ToolTurn { text: Some("You were testing intentd.".into()), tool_calls: vec![] }));
        let harness = Arc::new(Harness::for_client(Arc::new(client)));
        Providers {
            label: None,
            semantic: None,
            embedder: Arc::new(HashingEmbedder::new(256)),
            copilot: Some(Arc::clone(&harness)),
            harness: Some(harness),
            unavailable_reason: None,
        }
    });
    let d = TestDaemon::start_with(opts, launched).await;
    let (date, _) = seed_day(&d, "/home/dev/src/intent").await;
    d.call(Method::POST, &format!("/pipeline/run?date={date}"), None).await;
    let (s, body) =
        d.call(Method::POST, "/copilot/query", Some(json!({"question": "what was I testing?", "mode": "qa"}))).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert!(body["answer"].as_str().unwrap().contains("intentd"), "{body}");
    assert!(body["tool_calls_made"].as_array().unwrap().iter().any(|t| t == "search_intents"));
    d.stop().await;
}

#[tokio::test]
async fn embeddings_backfill_enables_hybrid_search() {
    let dir = tempfile::tempdir().unwrap();
    let d = TestDaemon::start(dir.path()).await;
    let (date, _) = seed_day(&d, "/home/dev/src/intent").await;
    d.call(Method::POST, &format!("/pipeline/run?date={date}"), None).await;
    let stored = intentd::jobs::backfill_once(&d.state).await;
    assert!(stored > 0, "offline hashing embedder fills the vector index");
    assert!(d.state.store.intents_missing_embeddings(10).unwrap().is_empty());
    let (_, intents) = d.call(Method::GET, &format!("/intents?date={date}"), None).await;
    let word = intents[0]["label"].as_str().unwrap().split_whitespace().next().unwrap().to_string();
    let (s, hits) = d.call(Method::GET, &format!("/intents/search?q={word}"), None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(hits[0]["score"].as_f64().unwrap() > 0.0);
    d.stop().await;
}

fn ids(intents: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for i in intents.as_array().into_iter().flatten() {
        out.extend(i["id"].as_str().map(str::to_owned));
        out.extend(ids(&i["children"]));
    }
    out
}

#[tokio::test]
async fn idle_rerun_keeps_intent_ids_and_embeddings() {
    let dir = tempfile::tempdir().unwrap();
    let d = TestDaemon::start(dir.path()).await;
    let (date, n) = seed_day(&d, "/home/dev/src/intent").await;
    let (_, first) = d.call(Method::POST, &format!("/pipeline/run?date={date}"), None).await;
    assert!(intentd::jobs::backfill_once(&d.state).await > 0);

    // The scheduler re-runs the day a few minutes into an idle period.
    let (_, t0) = local_ts(1, 9, 0);
    for (offset, kind) in [(1200, "idle_start"), (1500, "idle_end")] {
        let event = json!({"id": uuid(), "ts": t0 + offset, "source": "linux", "type": kind, "payload": {}});
        let (s, body) = d.post_event(&event).await;
        assert_eq!(s, StatusCode::CREATED, "{body}");
    }
    let (_, second) = d.call(Method::POST, &format!("/pipeline/run?date={date}"), None).await;
    assert_eq!(second["cached"], false);
    assert_ne!(first["source_hash"], second["source_hash"]);
    assert_eq!(ids(&first["intents"]), ids(&second["intents"]));

    // An open drawer or a preview from the morning notification still resolves.
    let root = first["intents"][0]["id"].as_str().unwrap();
    let (s, one) = d.call(Method::GET, &format!("/intents/{root}"), None).await;
    assert_eq!(s, StatusCode::OK, "{one}");
    let total: i64 =
        second["intents"].as_array().unwrap().iter().map(|i| i["stats"]["event_count"].as_i64().unwrap()).sum();
    assert_eq!(total as usize, n);
    // Unchanged intents keep their vectors instead of waiting for a re-embed.
    assert!(d.state.store.intents_missing_embeddings(10).unwrap().is_empty());
    d.stop().await;
}

#[tokio::test]
async fn misc_capture_routes() {
    let dir = tempfile::tempdir().unwrap();
    let d = TestDaemon::start(dir.path()).await;
    let (s, health) = d.call_open(Method::GET, "/healthz", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(health["ok"], true);
    assert_eq!(health["version"], "2.0.0");
    let (_, cfg) = d.call_open(Method::GET, "/v1/config", None).await;
    assert!(cfg["blocked_domains"].is_array());
    let (s, sample) = d.call(Method::GET, "/v1/intelligence/preview-sample", None).await;
    assert_eq!(s, StatusCode::OK);
    assert!(sample.is_object());
    let (s, purge) = d.call(Method::POST, "/v1/detailed-capture/purge", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(purge["ok"], true);
    assert_eq!(purge["deleted"], 0);
    let (s, cur) = d.call(Method::GET, "/intents/current", None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(cur, Value::Null);
    let (s, _) = d.call(Method::GET, "/intents/prediction", None).await;
    assert_eq!(s, StatusCode::OK);
    let (_, status) = d.call(Method::GET, "/v1/status", None).await;
    for key in ["collectors", "activity", "llm", "database", "paused_until", "detailed_capture", "services"] {
        assert!(status.get(key).is_some(), "status.{key} missing");
    }
    assert_eq!(status["llm"]["provider"], "none");
    d.stop().await;
}

/// POST a shell event with the given headers; returns the status.
async fn post_with(d: &TestDaemon, headers: &[(&str, &str)], body: String) -> StatusCode {
    let mut req = d.client.post(format!("{}/v1/event", d.base));
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    req.body(body).send().await.unwrap().status()
}

#[tokio::test]
async fn cross_site_writes_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let d = TestDaemon::start(dir.path()).await;
    let now = intent_store::now_ts();
    let body = || shell(now, "ls", "/", 0).to_string();
    let json = ("content-type", "application/json");

    // What fetch(..., {mode: "no-cors"}) sends from any web page.
    let s = post_with(&d, &[("origin", "https://evil.example"), ("content-type", "text/plain;charset=UTF-8")], body())
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    for origin in ["https://evil.example", "null", "http://localhost:5173", "http://127.0.0.1:9477"] {
        let s = post_with(&d, &[("origin", origin), json], body()).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{origin} must not write");
    }
    // Ingest needs a JSON content type, with or without an Origin.
    for ct in ["text/plain", "text/plain;charset=UTF-8", "application/x-www-form-urlencoded", "multipart/form-data"] {
        let s = post_with(&d, &[("content-type", ct)], body()).await;
        assert_eq!(s, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{ct}");
    }
    let s = post_with(&d, &[], body()).await;
    assert_eq!(s, StatusCode::UNSUPPORTED_MEDIA_TYPE);

    // The companions, the app and the shell hook keep working.
    let s = post_with(&d, &[("origin", "moz-extension://4b1c-any-uuid"), json], body()).await;
    assert_eq!(s, StatusCode::CREATED);
    let s = post_with(&d, &[("origin", "chrome-extension://abcdefghijklmnopabcdefghijklmnop"), json], body()).await;
    assert_eq!(s, StatusCode::CREATED);
    let s = post_with(&d, &[("origin", "tauri://localhost"), json], body()).await;
    assert_eq!(s, StatusCode::CREATED);
    let s = post_with(&d, &[("origin", "http://tauri.localhost"), json], body()).await;
    assert_eq!(s, StatusCode::CREATED);
    let s = post_with(&d, &[("content-type", "application/json; charset=utf-8")], body()).await;
    assert_eq!(s, StatusCode::CREATED);
    let s = post_with(&d, &[("content-type", "Application/JSON")], body()).await;
    assert_eq!(s, StatusCode::CREATED);

    // Every state-changing route checks the Origin, even with a valid token.
    let writes: &[(Method, &str)] = &[
        (Method::PUT, "/v1/workspaces"),
        (Method::POST, "/v1/capture/pause"),
        (Method::DELETE, "/v1/memory/date/2026-01-01"),
        (Method::POST, "/pipeline/run"),
    ];
    for (method, path) in writes {
        let resp = d
            .client
            .request(method.clone(), format!("{}{path}", d.base))
            .bearer_auth(&d.token)
            .header("origin", "https://evil.example")
            .json(&json!({"paused": true, "workspaces": []}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{method} {path}");
    }
    let (_, status) = d.call(Method::GET, "/v1/status", None).await;
    assert_eq!(status["capture_paused"], false);
    // Reads stay governed by CORS and the token, not by this rule.
    let resp = d.client.get(format!("{}/healthz", d.base)).header("origin", "https://evil.example").send().await;
    assert_eq!(resp.unwrap().status(), StatusCode::OK);
    d.stop().await;
}

#[tokio::test]
async fn foreign_host_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let d = TestDaemon::start(dir.path()).await;
    let get = |host: &'static str, path: &'static str| {
        d.client.get(format!("{}{path}", d.base)).header("host", host).bearer_auth(&d.token).send()
    };
    for host in [
        "evil.example:9477",
        "evil.example",
        "127.0.0.1.evil.example",
        "localhost.evil.example:9477",
        "[::2]:9477",
        "127.0.0.1:x",
    ] {
        for path in ["/healthz", "/v1/detailed-capture/config", "/v1/config", "/v1/status"] {
            let resp = get(host, path).await.unwrap();
            assert_eq!(resp.status(), StatusCode::MISDIRECTED_REQUEST, "Host {host:?} {path}");
        }
    }
    for host in ["127.0.0.1:9477", "127.0.0.1", "localhost:1", "LOCALHOST", "[::1]:9477", "[::1]"] {
        let resp = get(host, "/v1/detailed-capture/config").await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "Host {host:?}");
    }
    // A preflight for a foreign Host is refused too.
    let resp = d
        .client
        .request(Method::OPTIONS, format!("{}/v1/event", d.base))
        .header("host", "evil.example")
        .header("origin", "tauri://localhost")
        .header("access-control-request-method", "POST")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::MISDIRECTED_REQUEST);
    d.stop().await;
}

#[tokio::test]
async fn impossible_timestamps_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let d = TestDaemon::start(dir.path()).await;
    let now = intent_store::now_ts();
    for ts in [now * 1000, now + 400 * 86_400, now + 86_401] {
        let (s, body) = d.post_event(&shell(ts, "ls", "/", 0)).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "ts {ts}: {body}");
        assert!(body["detail"].as_str().unwrap().starts_with("ts must"), "{body}");
    }
    let (s, _) = d.post_event(&shell(now + 30, "ls", "/", 0)).await;
    assert_eq!(s, StatusCode::CREATED, "small clock skew is accepted");
    let (s, _) = d.post_event(&shell(now + 3600, "pwd", "/", 0)).await;
    assert_eq!(s, StatusCode::CREATED);
    d.stop().await;
}

#[tokio::test]
async fn pause_in_the_past_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let d = TestDaemon::start(dir.path()).await;
    let now = intent_store::now_ts();
    for until in [1, now - 60, now, i64::MIN] {
        let (s, body) = d.call(Method::POST, "/v1/capture/pause", Some(json!({"paused": true, "until": until}))).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "until {until}: {body}");
    }
    let (_, status) = d.call(Method::GET, "/v1/status", None).await;
    assert_eq!(status["capture_paused"], false);
    // A resume ignores `until`.
    let (s, _) = d.call(Method::POST, "/v1/capture/pause", Some(json!({"paused": false, "until": 1}))).await;
    assert_eq!(s, StatusCode::OK);
    d.stop().await;
}

#[tokio::test]
async fn odd_search_input_with_an_embedder() {
    let dir = tempfile::tempdir().unwrap();
    let d = TestDaemon::start(dir.path()).await;
    let (date, _) = seed_day(&d, "/home/dev/src/intent").await;
    d.call(Method::POST, &format!("/pipeline/run?date={date}"), None).await;
    assert!(intentd::jobs::backfill_once(&d.state).await > 0);
    for q in ["%2A", "x", "-", "%22%22", "a..b", "NEAR"] {
        let (s, hits) = d.call(Method::GET, &format!("/intents/search?q={q}"), None).await;
        assert_eq!(s, StatusCode::OK, "q={q}: {hits}");
        assert!(hits.is_array(), "q={q}: {hits}");
    }
    let (_, hits) = d.call(Method::GET, "/intents/search?q=%2A", None).await;
    assert_eq!(hits, json!([]));
    let (_, hits) = d.call(Method::GET, "/intents/search?q=zzqqxx", None).await;
    assert_eq!(hits, json!([]), "an unknown word must not return the nearest vectors");
    d.stop().await;
}
