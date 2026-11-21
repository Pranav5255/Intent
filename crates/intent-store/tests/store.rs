//! Integration tests for intent-store (each test uses its own temp directory).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Instant;

use intent_store::{DayZone, EmbedderSpec, FactInput, FactQuery, RetentionCutoffs, Store, StoreError, StoreOptions};
use intent_types::{
    EventCursor, Intent, IntentInsights, IntentStats, PipelineResult, PipelineWarning, RawEvent, ResumePayload,
    WarningLevel,
};
use rusqlite::{Connection, params};
use serde_json::json;
use tempfile::TempDir;

const DAY: i64 = 86_400;
/// 2024-01-01T00:00:00Z
const T0: i64 = 1_704_067_200;

fn open(dir: &TempDir) -> Store {
    open_with(dir, None)
}

fn open_with(dir: &TempDir, embedder: Option<EmbedderSpec>) -> Store {
    Store::open(
        dir.path().join("intent.db"),
        StoreOptions { day_zone: DayZone::Fixed(0), read_connections: 3, embedder, ..Default::default() },
    )
    .expect("open store")
}

fn ev(id: &str, ts: i64, source: &str, kind: &str) -> RawEvent {
    let payload = json!({ "n": id }).as_object().unwrap().clone();
    RawEvent {
        id: id.into(),
        ts,
        source: source.into(),
        kind: kind.into(),
        payload,
        schema_version: 1,
        ingested_at: None,
    }
}

fn ev_at(id: &str, ts: i64, ingested_at: i64) -> RawEvent {
    RawEvent { ingested_at: Some(ingested_at), ..ev(id, ts, "shell", "command") }
}

fn intent(id: &str, parent: Option<&str>, depth: i64, label: &str, summary: &str, tags: &[&str]) -> Intent {
    Intent {
        id: id.into(),
        parent_id: parent.map(Into::into),
        date: "2024-01-01".into(),
        label: label.into(),
        summary: summary.into(),
        confidence: 0.8,
        start_ts: T0 + depth * 10,
        end_ts: T0 + 600,
        depth,
        tags: tags.iter().map(|t| t.to_string()).collect(),
        stats: IntentStats { event_count: 5, duration_seconds: 600, ..Default::default() },
        insights: IntentInsights {
            shell: vec![json!({"command_family": "cargo", "count": 3}).as_object().unwrap().clone()],
            ..Default::default()
        },
        todos: vec![],
        evidence: vec![],
        resume_payload: ResumePayload { files: vec!["/src/main.rs".into()], ..Default::default() },
        prefix: None,
        semantic: None,
        privacy_policy_version: "safe-intent-features-v2".into(),
        children: vec![],
    }
}

fn result(intents: Vec<Intent>, hash: &str) -> PipelineResult {
    PipelineResult {
        intents,
        warnings: vec![PipelineWarning { level: WarningLevel::Warning, message: "w".into(), event_id: None }],
        source_hash: hash.into(),
        pipeline_version: "2.0".into(),
        cached: false,
    }
}

/// root(a) -> [child(b) -> [grandchild(c)], child(d)]
fn sample_tree() -> Vec<Intent> {
    let mut root =
        intent("a", None, 0, "Rust store work", "Implemented the sqlite store with FTS5", &["project:intent"]);
    let mut b = intent("b", Some("a"), 1, "Schema", "Wrote migrations", &["project:intent"]);
    b.children.push(intent("c", Some("b"), 2, "Triggers", "FTS triggers", &["project:intent"]));
    root.children.push(b);
    root.children.push(intent("d", Some("a"), 1, "Tests", "Wrote tests", &["project:intent"]));
    let other = intent("e", None, 0, "Browser research", "Read about reciprocal rank fusion", &["project:docs"]);
    vec![root, other]
}

#[test]
fn idempotent_insert_and_day_stats() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    let batch = vec![
        ev("1", T0 + 10, "shell", "command"),
        ev("2", T0 + 20, "shell", "command"),
        ev("3", T0 + DAY + 5, "vscode", "save"),
    ];
    let r = store.insert_events(&batch).unwrap();
    assert_eq!(r.inserted_ids().collect::<Vec<_>>(), vec!["1", "2", "3"]);
    assert_eq!(r.days.len(), 2);
    assert_eq!(r.days[0].date, "2024-01-01");
    assert_eq!(r.days[0].inserted, 2);

    // Re-send with one new event and one in-batch duplicate.
    let mut again = batch.clone();
    again.push(ev("4", T0 + 15, "shell", "command"));
    again.push(ev("4", T0 + 15, "shell", "command"));
    let r2 = store.insert_events(&again).unwrap();
    assert_eq!(r2.inserted_ids().collect::<Vec<_>>(), vec!["4"]);
    assert_eq!(r2.outcomes.len(), 5);
    assert!(!r2.outcomes[0].inserted);
    // Duplicates return the stored copy (with its original ingested_at).
    assert_eq!(r2.outcomes[0].event.ingested_at, r.outcomes[0].event.ingested_at);

    let meta = store.day_meta("2024-01-01").unwrap();
    assert_eq!(meta.event_count, 3);
    assert_eq!(meta.first_ts, Some(T0 + 10));
    assert_eq!(meta.last_ts, Some(T0 + 20));
    assert_eq!(meta.last_event_id.as_deref(), Some("2"));
    assert_eq!(meta.revision, 3);
    assert_eq!(meta.content_hash, "rev:3|count:3|last:2");
    let empty = store.day_meta("2023-12-31").unwrap();
    assert_eq!((empty.event_count, empty.revision), (0, 0));
    assert_eq!(empty.content_hash, "rev:0|count:0|last:");

    let ids: Vec<String> = store.events_for_day("2024-01-01").unwrap().into_iter().map(|e| e.id).collect();
    assert_eq!(ids, vec!["1", "4", "2"]);
    assert_eq!(store.events_since(T0 + 15).unwrap().len(), 3);
    assert!(matches!(store.events_for_day("2024-13-01"), Err(StoreError::InvalidArgument(_))));

    let stats = store.source_stats().unwrap();
    assert_eq!(stats.len(), 2);
    assert_eq!(stats[0].source, "shell");
    assert_eq!(stats[0].event_count, 3);
    assert_eq!(stats[0].last_event_ts, Some(T0 + 20));
}

#[test]
fn day_bucketing_uses_zone_at_insert_and_rebucket() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    // 23:30 UTC on Jan 1 is Jan 2 in UTC+1.
    let ts = T0 + 23 * 3600 + 1800;
    store.insert_event(&ev("x", ts, "shell", "command")).unwrap();
    assert_eq!(store.events_for_day("2024-01-01").unwrap().len(), 1);
    store.set_day_zone(DayZone::parse("+01:00").unwrap());
    assert_eq!(store.local_day(ts), "2024-01-02");
    // Existing rows keep their day until rebucketed.
    assert_eq!(store.events_for_day("2024-01-01").unwrap().len(), 1);
    assert_eq!(store.rebucket_days().unwrap(), 1);
    assert!(store.events_for_day("2024-01-01").unwrap().is_empty());
    assert_eq!(store.day_meta("2024-01-02").unwrap().event_count, 1);
    assert_eq!(store.day_meta("2024-01-01").unwrap().event_count, 0);
}

#[test]
fn pagination_cursor_follows_sort_order() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    // Same ts, ingested_at order disagrees with id order: the 1.x (ts, id) keyset
    // would skip rows here.
    let batch = vec![
        ev_at("z", T0 + 5, 100),
        ev_at("a", T0 + 5, 200),
        ev_at("m", T0 + 5, 150),
        ev_at("b", T0 + 1, 900),
        ev_at("y", T0 + 9, 50),
        ev_at("c", T0 + 9, 50),
        ev_at("q", T0 + 7, 1),
    ];
    store.insert_events(&batch).unwrap();
    let expected: Vec<String> = store.events_for_day("2024-01-01").unwrap().into_iter().map(|e| e.id).collect();
    assert_eq!(expected, vec!["b", "z", "m", "a", "q", "c", "y"]);

    let mut got = Vec::new();
    let mut cursor: Option<EventCursor> = None;
    let mut pages = 0;
    loop {
        let page = store.events_page("2024-01-01", cursor.as_ref(), 2).unwrap();
        pages += 1;
        assert_eq!(page.revision, 7);
        got.extend(page.events.iter().map(|e| e.id.clone()));
        if !page.has_more {
            assert!(page.next_cursor.is_none());
            break;
        }
        cursor = page.next_cursor;
    }
    assert_eq!(got, expected);
    assert_eq!(pages, 4);
    assert!(store.events_page("2024-01-01", None, 0).is_err());
    assert!(store.events_page("2024-01-01", None, 2001).is_err());
}

#[test]
fn purge_tiers_and_stat_recompute() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    let now = T0 + 100 * DAY;
    store
        .insert_events(&[
            // Day 1 (old): one detailed + one metadata event.
            ev("d1", T0 + 10, "vscode", "document_change"),
            ev("m1", T0 + 20, "shell", "command"),
            // Day 80: detailed (past detailed cutoff only) + metadata (kept).
            ev("d2", T0 + 80 * DAY, "firefox", "user_action"),
            ev("m2", T0 + 80 * DAY + 1, "linux", "app_focus"),
            // Recent: kept.
            ev("d3", now - 10, "filesystem", "file_content"),
            ev("m3", now - 5, "shell", "command"),
        ])
        .unwrap();
    let counts = store.detailed_event_counts().unwrap();
    assert_eq!(counts.get("vscode/document_change"), Some(&1));
    assert_eq!(counts.get("filesystem/file_content"), Some(&1));
    assert_eq!(counts.len(), 3);

    let cutoffs = RetentionCutoffs::from_days(Some(7), Some(30), now);
    let preview = store.retention_preview(&cutoffs).unwrap();
    assert_eq!((preview.detailed, preview.metadata, preview.total), (2, 1, 3));

    let day80 = store.local_day(T0 + 80 * DAY);
    let rev80_before = store.day_meta(&day80).unwrap().revision;
    let report = store.purge_retention(&cutoffs).unwrap();
    assert_eq!(report.deleted.detailed, 2);
    assert_eq!(report.deleted.metadata, 1);
    assert_eq!(report.affected_days, vec!["2024-01-01".to_string(), day80.clone()]);

    // Day 1 emptied -> stats row gone; day 80 recomputed with a bumped revision.
    assert_eq!(store.day_meta("2024-01-01").unwrap().event_count, 0);
    let m80 = store.day_meta(&day80).unwrap();
    assert_eq!(m80.event_count, 1);
    assert_eq!(m80.last_event_id.as_deref(), Some("m2"));
    assert_eq!(m80.first_ts, Some(T0 + 80 * DAY + 1));
    assert!(m80.revision > rev80_before);
    // Untouched day keeps its stats.
    assert_eq!(store.day_meta(&store.local_day(now - 5)).unwrap().event_count, 2);

    // Detailed cutoff never outlives the metadata cutoff.
    let c = RetentionCutoffs { detailed_before: None, metadata_before: Some(now) };
    assert_eq!(store.retention_preview(&c).unwrap().detailed, 1);

    let p = store.purge_detailed_events().unwrap();
    assert_eq!(p.deleted.detailed, 1);
    assert!(store.detailed_event_counts().unwrap().is_empty());
    assert_eq!(store.day_meta(&store.local_day(now - 5)).unwrap().event_count, 1);
}

#[test]
fn pipeline_save_reload_round_trip() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    let tree = sample_tree();
    store.save_pipeline_run("2024-01-01", &result(tree.clone(), "h1")).unwrap();

    assert_eq!(store.intents_for_date("2024-01-01").unwrap(), tree);
    let cached = store.cached_run("2024-01-01", "h1").unwrap().expect("cached");
    assert_eq!(cached.intents, tree);
    assert_eq!(cached.warnings.len(), 1);
    assert!(store.cached_run("2024-01-01", "other").unwrap().is_none());

    // Any node comes back with its own subtree.
    let b = store.intent_by_id("b").unwrap().unwrap();
    assert_eq!(b, tree[0].children[0]);
    assert_eq!(store.intent_by_id("a").unwrap().unwrap(), tree[0]);
    assert!(store.intent_by_id("nope").unwrap().is_none());

    // intent_json stores the node without children.
    let conn = Connection::open(store.path()).unwrap();
    let json: String = conn.query_row("SELECT intent_json FROM intents WHERE id = 'a'", [], |r| r.get(0)).unwrap();
    assert!(json.contains("\"children\":[]"));
    let project: String = conn.query_row("SELECT project FROM intents WHERE id = 'c'", [], |r| r.get(0)).unwrap();
    assert_eq!(project, "intent");

    let roots = store.root_intents(None).unwrap();
    assert_eq!(roots.len(), 2);
    assert!(roots.iter().all(|r| r.intent.children.is_empty()));

    let stats = store.stats("2024-01-01", "2024-01-31", None).unwrap();
    assert_eq!(stats.intent_count, 2);
    assert_eq!(stats.total_duration_seconds, 1200);
    assert_eq!(stats.event_count, 10);
    assert_eq!(stats.by_date.len(), 1);
    assert_eq!(stats.projects.len(), 2);
    let s2 = store.stats("2024-01-01", "2024-01-01", Some("project:intent")).unwrap();
    assert_eq!(s2.intent_count, 1);
    assert_eq!(s2.top_labels[0].label, "Rust store work");
    assert!(store.stats("2024-02-01", "2024-01-01", None).is_err());

    // Re-saving replaces the date atomically.
    store.save_pipeline_run("2024-01-01", &result(vec![tree[1].clone()], "h2")).unwrap();
    assert_eq!(store.intents_for_date("2024-01-01").unwrap(), vec![tree[1].clone()]);
    assert!(store.cached_run("2024-01-01", "h1").unwrap().is_none());
    assert!(store.search("migrations", 10, None, None).unwrap().is_empty());

    // delete_date clears a date but keeps nothing searchable.
    let del = store.delete_date("2024-01-01").unwrap();
    assert_eq!(del.deleted_count, 1);
    assert!(store.intents_for_date("2024-01-01").unwrap().is_empty());
    assert!(store.search("fusion", 10, None, None).unwrap().is_empty());

    // forget_project reports the project's trees and clears the affected date.
    store.save_pipeline_run("2024-01-01", &result(tree.clone(), "h3")).unwrap();
    let del = store.forget_project("intent", &[]).unwrap();
    assert_eq!(del.deleted_intent_ids, vec!["a", "b", "c", "d"]);
    assert_eq!(del.affected_dates, vec!["2024-01-01"]);
    assert!(store.cached_run("2024-01-01", "h3").unwrap().is_none());
    assert!(store.intents_for_date("2024-01-01").unwrap().is_empty());
}

#[test]
fn fts_search_and_escaping() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    store.save_pipeline_run("2024-01-01", &result(sample_tree(), "h")).unwrap();

    let hits = store.search("sqlite store", 10, None, None).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, "a");
    assert!(hits[0].highlight_snippet.contains("**"), "{}", hits[0].highlight_snippet);
    assert!(hits[0].score > 0.0);

    // Prefix match on the last token (search-as-you-type).
    assert_eq!(store.search("reciprocal fus", 10, None, None).unwrap()[0].id, "e");
    // Children are not search results; tags and safe insights are indexed.
    assert!(store.search("Triggers", 10, None, None).unwrap().is_empty());
    assert_eq!(store.search("project:docs", 10, None, None).unwrap()[0].id, "e");
    assert_eq!(store.search("cargo", 10, None, None).unwrap().len(), 2);
    // AND falls back to OR when nothing matches every token.
    assert_eq!(store.search("fusion zebra", 10, None, None).unwrap()[0].id, "e");
    // Date filter.
    assert!(store.search("fusion", 10, Some("2024-01-02"), None).unwrap().is_empty());
    assert!(store.search("fusion", 10, Some("2024-01-02"), Some("2024-01-01")).is_err());

    for nasty in [
        r#"foo" OR bar*("#,
        "\"",
        "*",
        "NEAR(",
        "label:sqlite",
        "-store",
        "AND OR NOT",
        "^sqlite",
        "'; DROP TABLE intents; --",
        "\u{0}\u{1}",
        "",
        "   ",
        "((((",
        "sqlite\"*",
    ] {
        let r = store.search(nasty, 10, None, None);
        assert!(r.is_ok(), "query {nasty:?} failed: {r:?}");
    }
    assert_eq!(store.search("label:sqlite", 10, None, None).unwrap()[0].id, "a");
    // Porter stemming: "intents" matches the `project:intent` tag (OR fallback), safely.
    assert_eq!(store.search("'; DROP TABLE intents; --", 10, None, None).unwrap()[0].id, "a");
    assert_eq!(store.intents_for_date("2024-01-01").unwrap().len(), 2);
}

#[test]
fn vec_round_trip_and_rebuild() {
    let dir = TempDir::new().unwrap();
    let spec = EmbedderSpec { identity: "test:3d".into(), dims: 3 };
    let store = open_with(&dir, Some(spec.clone()));
    store.save_pipeline_run("2024-01-01", &result(sample_tree(), "h")).unwrap();

    assert_eq!(store.intents_missing_embeddings(10).unwrap().len(), 2);
    assert!(store.upsert_embedding("a", &[1.0, 0.0, 0.0]).unwrap());
    assert!(store.upsert_embedding("a", &[0.5, 0.25, 0.0]).unwrap()); // replace
    assert!(!store.upsert_embedding("missing", &[1.0, 0.0, 0.0]).unwrap());
    assert!(matches!(store.upsert_embedding("a", &[1.0, 0.0]), Err(StoreError::InvalidArgument(_))));
    assert!(store.upsert_embedding("a", &[f32::NAN, 0.0, 0.0]).is_err());
    assert_eq!(store.embedding("a").unwrap(), Some(vec![0.5, 0.25, 0.0]));
    assert_eq!(store.intents_missing_embeddings(10).unwrap().len(), 1);

    // Unchanged nodes keep their vector across a re-save; changed ones lose it.
    let mut tree = sample_tree();
    store.upsert_embedding("e", &[0.0, 1.0, 0.0]).unwrap();
    tree[1].summary = "changed".into();
    store.save_pipeline_run("2024-01-01", &result(tree, "h2")).unwrap();
    assert_eq!(store.embedding("a").unwrap(), Some(vec![0.5, 0.25, 0.0]));
    assert_eq!(store.embedding("e").unwrap(), None);

    // Reopening with the same spec keeps vectors; a new identity rebuilds.
    drop(store);
    let store = open_with(&dir, Some(spec.clone()));
    assert!(store.embedding("a").unwrap().is_some());
    assert!(!store.configure_embeddings(&spec).unwrap());
    let spec4 = EmbedderSpec { identity: "test:4d".into(), dims: 4 };
    assert!(store.configure_embeddings(&spec4).unwrap());
    assert_eq!(store.embedder().unwrap(), Some(spec4));
    assert_eq!(store.embedding("a").unwrap(), None);
    assert!(store.upsert_embedding("a", &[0.0, 0.0, 0.0, 1.0]).unwrap());

    // No embedder configured -> vectors are refused.
    let dir2 = TempDir::new().unwrap();
    let plain = open(&dir2);
    assert!(matches!(plain.upsert_embedding("a", &[1.0]), Err(StoreError::EmbeddingsDisabled)));
}

#[test]
fn hybrid_search_ordering() {
    let dir = TempDir::new().unwrap();
    let store = open_with(&dir, Some(EmbedderSpec { identity: "t".into(), dims: 3 }));
    let a = intent("a", None, 0, "Kubernetes deploy", "Rolled out the cluster upgrade", &[]);
    let b = intent("b", None, 0, "Kubernetes debugging", "Investigated pod crash loops", &[]);
    let c = intent("c", None, 0, "Container networking", "Read CNI documentation", &[]);
    store.save_pipeline_run("2024-01-01", &result(vec![a, b, c], "h")).unwrap();
    store.upsert_embedding("a", &[0.0, 0.0, 1.0]).unwrap();
    store.upsert_embedding("b", &[0.9, 0.1, 0.0]).unwrap();
    store.upsert_embedding("c", &[1.0, 0.0, 0.0]).unwrap();

    let q = [1.0f32, 0.05, 0.0];
    // Keyword only: a and b match "kubernetes".
    let kw = store.hybrid_search("kubernetes", None, 10, None, None).unwrap();
    assert_eq!(kw.len(), 2);
    // Vector only.
    let vs = store.vector_search(&q, 10, None, None).unwrap();
    assert_eq!(vs.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), vec!["c", "b", "a"]);
    assert!(vs[0].score > vs[2].score);
    // Fused: b is in both lists and wins; c (vector #1) beats a (keyword-only).
    let hy = store.hybrid_search("kubernetes", Some(&q), 10, None, None).unwrap();
    let ids: Vec<&str> = hy.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(ids[0], "b");
    assert_eq!(ids.len(), 3);
    assert!(hy.windows(2).all(|w| w[0].score >= w[1].score));
    assert!(hy[0].highlight_snippet.contains("**"));
    assert_eq!(store.hybrid_search("kubernetes", Some(&q), 1, None, None).unwrap().len(), 1);
    assert!(store.hybrid_search("x", Some(&[1.0]), 5, None, None).is_err());
}

#[test]
fn hybrid_search_ignores_empty_and_distant_vectors() {
    let dir = TempDir::new().unwrap();
    let store = open_with(&dir, Some(EmbedderSpec { identity: "t".into(), dims: 3 }));
    let a = intent("a", None, 0, "Kubernetes deploy", "Rolled out the cluster upgrade", &[]);
    let b = intent("b", None, 0, "Container networking", "Read CNI documentation", &[]);
    let z = intent("z", None, 0, "Empty vector", "An intent whose text embedded to nothing", &[]);
    store.save_pipeline_run("2024-01-01", &result(vec![a, b, z], "h")).unwrap();
    store.upsert_embedding("a", &[1.0, 0.0, 0.0]).unwrap();
    store.upsert_embedding("b", &[0.0, 1.0, 0.0]).unwrap();
    store.upsert_embedding("z", &[0.0, 0.0, 0.0]).unwrap();

    // A zero-norm query (text with no tokens) has no cosine distance: no semantic hits.
    assert!(store.hybrid_search("*", Some(&[0.0, 0.0, 0.0]), 10, None, None).unwrap().is_empty());
    assert!(store.vector_search(&[0.0, 0.0, 0.0], 10, None, None).unwrap().is_empty());
    // A non-finite query falls back to keyword ranking.
    let hits = store.hybrid_search("kubernetes", Some(&[f32::NAN, 0.0, 0.0]), 10, None, None).unwrap();
    assert_eq!(hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), vec!["a"]);
    // A stored zero vector has a NULL distance: it is no match, not an error.
    let hits = store.vector_search(&[1.0, 0.0, 0.0], 10, None, None).unwrap();
    assert!(hits.iter().all(|h| h.id != "z"), "{hits:?}");
    // KNN always returns the nearest vectors; semantic-only hits need some similarity.
    assert!(store.hybrid_search("zzqqxx", Some(&[0.0, 0.0, 1.0]), 10, None, None).unwrap().is_empty());
    let hits = store.hybrid_search("zzqqxx", Some(&[0.1, 1.0, 0.0]), 10, None, None).unwrap();
    assert_eq!(hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), vec!["b"]);
    // A keyword match stays even when its vector is far away.
    let hits = store.hybrid_search("kubernetes", Some(&[0.0, 1.0, 0.0]), 10, None, None).unwrap();
    assert_eq!(hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
}

#[test]
fn facts_and_metadata() {
    let dir = TempDir::new().unwrap();
    let store = open_with(&dir, Some(EmbedderSpec { identity: "t".into(), dims: 2 }));
    let f = FactInput {
        subject: "user".into(),
        predicate: "prefers".into(),
        object: "Rust".into(),
        source_intent_id: Some("a".into()),
        seen_ts: 100,
        confidence: 0.6,
        embedding: Some(vec![1.0, 0.0]),
    };
    let id = store.upsert_fact(&f).unwrap();
    let id2 = store
        .upsert_fact(&FactInput { object: "rust".into(), seen_ts: 200, confidence: 0.9, embedding: None, ..f.clone() })
        .unwrap();
    assert_eq!(id, id2, "case-insensitive dedupe");
    store
        .upsert_fact(&FactInput {
            predicate: "uses".into(),
            object: "100%_vim".into(),
            seen_ts: 50,
            embedding: Some(vec![0.0, 1.0]),
            ..f.clone()
        })
        .unwrap();
    let all = store.facts(&FactQuery::default()).unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].id, id);
    assert_eq!((all[0].first_seen_ts, all[0].last_seen_ts, all[0].confidence), (100, 200, 0.9));
    assert_eq!(store.facts(&FactQuery { text: Some("100%_".into()), ..Default::default() }).unwrap().len(), 1);
    assert_eq!(store.facts(&FactQuery { text: Some("%".into()), ..Default::default() }).unwrap().len(), 1);
    assert_eq!(store.facts(&FactQuery { subject: Some("USER".into()), ..Default::default() }).unwrap().len(), 2);
    let sim = store.similar_facts(&[0.9, 0.1], 1).unwrap();
    assert_eq!(sim[0].0.id, id);
    assert!(store.upsert_fact(&FactInput { subject: " ".into(), ..f.clone() }).is_err());
    assert!(store.delete_fact(id).unwrap());
    assert_eq!(store.clear_facts().unwrap(), 1);

    assert_eq!(store.get_metadata("k").unwrap(), None);
    store.set_metadata("k", "v").unwrap();
    store.set_metadata_many(&[("k", "v2"), ("j", "1")]).unwrap();
    assert_eq!(store.get_metadata("k").unwrap().as_deref(), Some("v2"));
    assert!(store.delete_metadata("j").unwrap());
    let cur = EventCursor { after_ts: 5, after_id: "x".into() };
    store.set_ingest_cursor("engine", &cur).unwrap();
    assert_eq!(store.ingest_cursor("engine").unwrap(), Some(cur));
}

/// Build 1.x-shaped databases exactly as the Python services created them.
fn make_legacy(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let events = dir.join("events.db");
    let c = Connection::open(&events).unwrap();
    c.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL);
         CREATE TABLE events (id TEXT PRIMARY KEY, schema_version INTEGER NOT NULL DEFAULT 1, ts INTEGER NOT NULL,
             source TEXT NOT NULL, type TEXT NOT NULL, payload TEXT NOT NULL, ingested_at INTEGER NOT NULL);
         CREATE TABLE day_export_stats (date TEXT PRIMARY KEY, event_count INTEGER NOT NULL, first_ts INTEGER,
             last_ts INTEGER, last_event_id TEXT, revision INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL);",
    )
    .unwrap();
    for i in 0..50 {
        c.execute(
            "INSERT INTO events VALUES (?1, 1, ?2, 'shell', 'command', ?3, ?4)",
            params![format!("ev{i}"), T0 + i * 3600, r#"{"cmd":"ls","cwd":"/"}"#, T0 + i * 3600 + 1],
        )
        .unwrap();
    }
    c.execute("INSERT INTO events VALUES ('bad', 1, ?1, 'shell', 'command', 'not json', ?1)", [T0]).unwrap();
    drop(c);

    let intents = dir.join("intents.db");
    let c = Connection::open(&intents).unwrap();
    c.execute_batch(
        "CREATE TABLE pipeline_runs (date TEXT, source_hash TEXT, status TEXT, warnings_json TEXT,
             started_at INTEGER, completed_at INTEGER, PRIMARY KEY (date, source_hash));
         CREATE TABLE intents (id TEXT PRIMARY KEY, date TEXT, parent_id TEXT NULL, start_ts INTEGER, end_ts INTEGER,
             label TEXT, summary TEXT, depth INTEGER, intent_json TEXT, source_hash TEXT, created_at INTEGER);
         CREATE TABLE store_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE VIRTUAL TABLE intent_search USING fts5(id UNINDEXED, label, summary, insights, tags);",
    )
    .unwrap();
    c.execute("INSERT INTO pipeline_runs VALUES ('2024-01-01', 'lh', 'complete', '[]', ?1, ?1)", [T0]).unwrap();
    for node in sample_tree().iter().flat_map(|r| r.walk()) {
        // Python stored nodes with children emptied, plus unknown extra fields.
        let mut v = serde_json::to_value(Intent { children: vec![], ..node.clone() }).unwrap();
        v["python_only_field"] = json!(true);
        c.execute(
            "INSERT INTO intents VALUES (?1, '2024-01-01', ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'lh', ?9)",
            params![
                node.id,
                node.parent_id,
                node.start_ts,
                node.end_ts,
                node.label,
                node.summary,
                node.depth,
                v.to_string(),
                T0
            ],
        )
        .unwrap();
    }
    c.execute("INSERT INTO intents VALUES ('broken', '2024-01-02', NULL, 0, 0, 'x', 'y', 0, '{oops', 'lh', 0)", [])
        .unwrap();
    (events, intents)
}

#[test]
fn legacy_import_is_idempotent() {
    let dir = TempDir::new().unwrap();
    let (events_db, intents_db) = make_legacy(dir.path());
    let store = open(&dir);
    store.insert_event(&ev("ev0", T0, "shell", "command")).unwrap(); // pre-existing overlap

    let r = store.import_legacy(&events_db, &intents_db).unwrap();
    assert!(r.events_db_found && r.intents_db_found);
    assert_eq!(r.events_imported, 49);
    assert_eq!(r.events_skipped, 1);
    assert_eq!(r.intents_imported, 5);
    assert_eq!(r.intents_skipped, 1);
    assert_eq!(r.runs_imported, 1);

    // 50 hourly events from 00:00 UTC Jan 1 -> 24 on Jan 1, 24 on Jan 2, 2 on Jan 3.
    assert_eq!(store.day_meta("2024-01-01").unwrap().event_count, 24);
    assert_eq!(store.day_meta("2024-01-02").unwrap().event_count, 24);
    assert_eq!(store.events_for_day("2024-01-03").unwrap().len(), 2);
    assert_eq!(store.events_for_day("2024-01-02").unwrap()[0].payload["cmd"], "ls");

    // Trees rebuild (sibling order from 1.x start_ts, id) and are searchable.
    let trees = store.intents_for_date("2024-01-01").unwrap();
    assert_eq!(trees.len(), 2);
    assert_eq!(trees[0].walk().len(), 4);
    assert!(store.cached_run("2024-01-01", "lh").unwrap().is_some());
    assert_eq!(store.search("sqlite", 5, None, None).unwrap()[0].id, "a");
    assert_eq!(store.stats("2024-01-01", "2024-01-01", Some("intent")).unwrap().intent_count, 1);

    let again = store.import_legacy(&events_db, &intents_db).unwrap();
    assert!(again.events_already_imported && again.intents_already_imported);
    assert_eq!(again.events_imported + again.intents_imported, 0);

    // Missing files are a no-op.
    let none = store.import_legacy(&dir.path().join("nope.db"), &dir.path().join("nope2.db")).unwrap();
    assert!(!none.events_db_found && !none.intents_db_found);
}

#[test]
fn concurrent_writer_and_readers() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    let done = Arc::new(AtomicBool::new(false));
    let mut readers = Vec::new();
    for _ in 0..4 {
        let store = store.clone();
        let done = Arc::clone(&done);
        readers.push(thread::spawn(move || {
            let mut reads = 0u64;
            let mut last_count = 0;
            while !done.load(Ordering::Relaxed) {
                let meta = store.day_meta("2024-01-01").unwrap();
                assert!(meta.event_count >= last_count, "counts never go backwards");
                last_count = meta.event_count;
                let page = store.events_page("2024-01-01", None, 50).unwrap();
                assert!(page.events.len() as i64 <= meta.event_count.max(50));
                store.source_stats().unwrap();
                reads += 1;
            }
            reads
        }));
    }
    let writers: Vec<_> = (0..2)
        .map(|w| {
            let store = store.clone();
            thread::spawn(move || {
                for b in 0..50 {
                    let batch: Vec<RawEvent> = (0..20)
                        .map(|i| ev(&format!("w{w}-{b}-{i}"), T0 + (b * 20 + i) as i64, "shell", "command"))
                        .collect();
                    store.insert_events(&batch).unwrap();
                }
            })
        })
        .collect();
    for w in writers {
        w.join().unwrap();
    }
    done.store(true, Ordering::Relaxed);
    let total_reads: u64 = readers.into_iter().map(|r| r.join().unwrap()).sum();
    assert!(total_reads > 0);
    let meta = store.day_meta("2024-01-01").unwrap();
    assert_eq!(meta.event_count, 2000);
    assert_eq!(meta.revision, 2000);
    assert_eq!(store.events_for_day("2024-01-01").unwrap().len(), 2000);
}

/// `cargo test -p intent-store --release -- --ignored --nocapture bench_insert_100k`
#[test]
#[ignore]
fn bench_insert_100k() {
    let dir = TempDir::new().unwrap();
    let store = open(&dir);
    let events: Vec<RawEvent> = (0..100_000)
        .map(|i| {
            let mut e = ev(&format!("evt-{i:06}"), T0 + i as i64 * 3, "vscode", "document_change");
            e.payload.insert("path".into(), json!(format!("/home/u/project/src/file_{}.rs", i % 300)));
            e
        })
        .collect();
    let start = Instant::now();
    for chunk in events.chunks(1_000) {
        store.insert_events(chunk).unwrap();
    }
    let secs = start.elapsed().as_secs_f64();
    println!("inserted 100000 events in {secs:.2}s ({:.0} events/s, batches of 1000)", 100_000.0 / secs);
    let start = Instant::now();
    let n = store.events_for_day("2024-01-02").unwrap().len();
    println!("events_for_day ({n} rows) in {:.1} ms", start.elapsed().as_secs_f64() * 1e3);
    let start = Instant::now();
    let r = store
        .purge_retention(&RetentionCutoffs { detailed_before: Some(T0 + 150_000), metadata_before: None })
        .unwrap();
    println!(
        "purged {} rows across {} days in {:.1} ms",
        r.deleted.total,
        r.affected_days.len(),
        start.elapsed().as_secs_f64() * 1e3
    );
}
