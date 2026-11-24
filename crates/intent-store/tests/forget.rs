//! Forgetting and retention: deleted data must not come back and must leave the files.

use intent_store::{DayZone, RetentionCutoffs, Store, StoreOptions};
use intent_types::{Intent, IntentInsights, IntentStats, PipelineResult, RawEvent, ResumePayload};
use serde_json::json;
use tempfile::TempDir;

const DAY: i64 = 86_400;
/// 2024-01-01T00:00:00Z
const T0: i64 = 1_704_067_200;

fn open(dir: &TempDir) -> Store {
    Store::open(
        dir.path().join("intent.db"),
        StoreOptions { day_zone: DayZone::Fixed(0), read_connections: 2, ..Default::default() },
    )
    .expect("open store")
}

fn tab(id: &str, ts: i64, url: &str) -> RawEvent {
    RawEvent {
        id: id.into(),
        ts,
        source: "firefox".into(),
        kind: "tab_change".into(),
        payload: json!({"url": url, "title": url}).as_object().unwrap().clone(),
        schema_version: 1,
        ingested_at: None,
    }
}

fn node(id: &str, date: &str, parent: Option<&str>, label: &str, tags: &[&str]) -> Intent {
    Intent {
        id: id.into(),
        parent_id: parent.map(Into::into),
        date: date.into(),
        label: label.into(),
        summary: format!("{label} summary"),
        confidence: 0.5,
        start_ts: T0,
        end_ts: T0 + 60,
        depth: i64::from(parent.is_some()),
        tags: tags.iter().map(|t| t.to_string()).collect(),
        stats: IntentStats::default(),
        insights: IntentInsights::default(),
        todos: vec![],
        evidence: vec![],
        resume_payload: ResumePayload::default(),
        prefix: None,
        semantic: None,
        privacy_policy_version: "safe-intent-features-v2".into(),
        children: vec![],
    }
}

fn run(intents: Vec<Intent>, hash: &str) -> PipelineResult {
    PipelineResult {
        intents,
        warnings: vec![],
        source_hash: hash.into(),
        pipeline_version: "0.2".into(),
        cached: false,
    }
}

/// Every byte of the database and its WAL.
fn file_bytes(store: &Store) -> Vec<u8> {
    let mut out = std::fs::read(store.path()).unwrap();
    let wal = store.path().with_extension("db-wal");
    if let Ok(bytes) = std::fs::read(wal) {
        out.extend(bytes);
    }
    out
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle.as_bytes())
}

#[test]
fn forget_date_deletes_events_stats_and_intents() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    store
        .insert_events(&[tab("a", T0 + 10, "https://bank.example/"), tab("b", T0 + DAY, "https://keep.example/")])
        .unwrap();
    let mut root = node("r1", "2024-01-01", None, "Bank statement", &[]);
    root.children.push(node("c1", "2024-01-01", Some("r1"), "Bank login", &[]));
    store.save_pipeline_run("2024-01-01", &run(vec![root], "h1")).unwrap();
    store.save_pipeline_run("2024-01-02", &run(vec![node("r2", "2024-01-02", None, "Keep this", &[])], "h2")).unwrap();

    let report = store.forget_date("2024-01-01").unwrap();
    assert_eq!(report.deleted_count, 2);
    assert_eq!(report.deleted_event_count, 1);
    assert_eq!(report.affected_dates, vec!["2024-01-01".to_string()]);

    assert!(store.events_for_day("2024-01-01").unwrap().is_empty());
    assert_eq!(store.day_meta("2024-01-01").unwrap().event_count, 0);
    assert!(store.intents_for_date("2024-01-01").unwrap().is_empty());
    assert!(store.cached_run("2024-01-01", "h1").unwrap().is_none());
    assert!(store.search("bank", 10, None, None).unwrap().is_empty());
    // The next day is untouched.
    assert_eq!(store.events_for_day("2024-01-02").unwrap().len(), 1);
    assert_eq!(store.intents_for_date("2024-01-02").unwrap().len(), 1);
}

#[test]
fn forget_project_deletes_its_events_and_every_intent_of_the_affected_days() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    store
        .insert_events(&[
            tab("p1", T0 + 10, "https://p.example/"),
            tab("o1", T0 + 20, "https://other.example/"),
            tab("k1", T0 + 2 * DAY, "https://keep.example/"),
        ])
        .unwrap();
    // A mixed session: the parent's first tag is another project.
    let mut mixed = node("m", "2024-01-01", None, "Mixed session", &["project:other", "project:zorblax"]);
    mixed.children.push(node("m1", "2024-01-01", Some("m"), "Other work", &["project:other"]));
    mixed.children.push(node("m2", "2024-01-01", Some("m"), "Zorblax work", &["project:zorblax"]));
    store.save_pipeline_run("2024-01-01", &run(vec![mixed], "h1")).unwrap();
    store
        .save_pipeline_run("2024-01-02", &run(vec![node("z", "2024-01-02", None, "More", &["project:zorblax"])], "h2"))
        .unwrap();
    store
        .save_pipeline_run("2024-01-03", &run(vec![node("k", "2024-01-03", None, "Keep", &["project:keep"])], "h3"))
        .unwrap();

    let report = store.forget_project("project:zorblax", &["p1".to_string()]).unwrap();
    assert_eq!(report.deleted_event_count, 1);
    assert_eq!(report.deleted_intent_ids, vec!["m2", "z"]);
    assert_eq!(report.affected_dates, vec!["2024-01-01".to_string(), "2024-01-02".to_string()]);

    let events = store.events_for_day("2024-01-01").unwrap();
    assert_eq!(events.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), vec!["o1"]);
    assert_eq!(store.day_meta("2024-01-01").unwrap().event_count, 1);
    // The mixed parent is gone too; the caller rebuilds the day from the events left.
    assert!(store.intents_for_date("2024-01-01").unwrap().is_empty());
    assert!(store.intents_for_date("2024-01-02").unwrap().is_empty());
    assert!(store.search("zorblax", 10, None, None).unwrap().is_empty());
    assert_eq!(store.intents_for_date("2024-01-03").unwrap().len(), 1);
}

#[test]
fn events_mentioning_finds_the_text_in_payloads() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    store.insert_events(&[tab("a", T0, "https://zorblax.example/"), tab("b", T0 + 1, "https://x.example/")]).unwrap();
    let hits = store.events_mentioning("zorblax").unwrap();
    assert_eq!(hits.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), vec!["a"]);
    assert!(store.events_mentioning("").is_err());
}

#[test]
fn retention_removes_intents_of_days_past_the_metadata_cutoff() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    let now = T0 + 40 * DAY;
    store
        .insert_events(&[tab("old", T0 + 10, "https://old.example/"), tab("new", now - 10, "https://new.example/")])
        .unwrap();
    let mut old = node("o", "2024-01-01", None, "Ancient axum work", &[]);
    old.children.push(node("o1", "2024-01-01", Some("o"), "Axum routing", &[]));
    store.save_pipeline_run("2024-01-01", &run(vec![old], "h1")).unwrap();
    let today = store.local_day(now);
    store.save_pipeline_run(&today, &run(vec![node("n", &today, None, "Fresh axum work", &[])], "h2")).unwrap();

    let cutoffs = RetentionCutoffs::from_days(None, Some(30), now);
    let preview = store.retention_preview(&cutoffs).unwrap();
    assert_eq!((preview.metadata, preview.intents), (1, 2));

    let report = store.purge_retention(&cutoffs).unwrap();
    assert_eq!(report.deleted.intents, 2);
    assert!(store.intents_for_date("2024-01-01").unwrap().is_empty());
    assert!(store.cached_run("2024-01-01", "h1").unwrap().is_none());
    let hits = store.search("axum", 10, None, None).unwrap();
    assert_eq!(hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), vec!["n"]);
}

#[test]
fn forgotten_text_leaves_the_database_and_wal_files() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    // Distinct first letters keep FTS5 from prefix-compressing the terms away.
    store
        .insert_events(&[
            tab("a", T0 + 10, "https://qwvbank.example/"),
            tab("b", T0 + 2 * DAY, "https://kept.example/"),
        ])
        .unwrap();
    store
        .save_pipeline_run("2024-01-01", &run(vec![node("r", "2024-01-01", None, "Jyxstatement", &[])], "h1"))
        .unwrap();
    store.save_pipeline_run("2024-01-03", &run(vec![node("k", "2024-01-03", None, "Mpold", &[])], "h3")).unwrap();
    let before = file_bytes(&store);
    assert!(contains(&before, "qwvbank") && contains(&before, "Jyxstatement"));

    store.forget_date("2024-01-01").unwrap();
    let after = file_bytes(&store);
    assert!(!contains(&after, "qwvbank"), "a forgotten event is still on disk");
    assert!(!contains(&after, "Jyxstatement"), "a forgotten intent is still on disk");
    assert!(!contains(&after, "jyxsta"), "a forgotten search term is still on disk");

    store.purge_retention(&RetentionCutoffs { detailed_before: None, metadata_before: Some(T0 + 3 * DAY) }).unwrap();
    let after = file_bytes(&store);
    assert!(!contains(&after, "kept.example"), "a purged event is still on disk");
    assert!(!contains(&after, "Mpold"), "a purged intent is still on disk");
}
