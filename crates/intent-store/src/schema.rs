//! Versioned schema migrations.
//!
//! Each migration runs once, inside its own transaction, and is recorded in
//! `schema_migrations`. Never edit a released migration; append a new one.
//!
//! The sqlite-vec table (`intent_vec`) is *not* created here because its dimension is
//! only known at runtime; see [`crate::search`] (`configure_embeddings`).

use rusqlite::{Connection, OptionalExtension, params};

use crate::day::now_ts;
use crate::error::Result;

/// Ordered list of `(version, sql)` migrations.
const MIGRATIONS: &[(i64, &str)] = &[(1, V1)];

/// Initial 2.0 schema: merged 1.x `events.db` + `intents.db` plus search/memory tables.
const V1: &str = r#"
-- Raw capture events. `day` is the calendar day computed at insert time from the
-- configured DayZone so reads never depend on the reader's time zone.
CREATE TABLE events (
    id              TEXT PRIMARY KEY,
    schema_version  INTEGER NOT NULL DEFAULT 1,
    ts              INTEGER NOT NULL,
    source          TEXT NOT NULL,
    type            TEXT NOT NULL,
    payload         TEXT NOT NULL,
    ingested_at     INTEGER NOT NULL,
    day             TEXT NOT NULL
);
CREATE INDEX idx_events_ts ON events(ts, ingested_at, id);
-- Superset of (source, type, ts): also covers MAX(ingested_at) for source stats.
CREATE INDEX idx_events_source_type_ts ON events(source, type, ts, ingested_at);
CREATE INDEX idx_events_day ON events(day, ts, ingested_at, id);

-- Per-day aggregate maintained transactionally on insert/purge (feeds /v1/export/day/meta).
CREATE TABLE day_stats (
    date              TEXT PRIMARY KEY,
    event_count       INTEGER NOT NULL,
    first_ts          INTEGER,
    last_ts           INTEGER,
    last_event_id     TEXT,
    last_ingested_at  INTEGER,
    revision          INTEGER NOT NULL DEFAULT 0,
    updated_at        INTEGER NOT NULL
);

CREATE TABLE pipeline_runs (
    date           TEXT NOT NULL,
    source_hash    TEXT NOT NULL,
    status         TEXT NOT NULL,
    warnings_json  TEXT NOT NULL DEFAULT '[]',
    started_at     INTEGER NOT NULL,
    completed_at   INTEGER,
    PRIMARY KEY (date, source_hash)
);

-- One row per intent node (children are rows too, linked by parent_id).
-- `pk` is an explicit INTEGER PRIMARY KEY so VACUUM never renumbers the rowids
-- that intent_fts (external content) and intent_vec are keyed by.
CREATE TABLE intents (
    pk                INTEGER PRIMARY KEY,
    id                TEXT NOT NULL UNIQUE,
    date              TEXT NOT NULL,
    parent_id         TEXT,
    ord               INTEGER NOT NULL,          -- pre-order position within the run
    start_ts          INTEGER NOT NULL,
    end_ts            INTEGER NOT NULL,
    label             TEXT NOT NULL,
    summary           TEXT NOT NULL,
    depth             INTEGER NOT NULL,
    project           TEXT,                      -- from the `project:<name>` tag
    confidence        REAL NOT NULL DEFAULT 0,
    event_count       INTEGER NOT NULL DEFAULT 0,
    duration_seconds  INTEGER NOT NULL DEFAULT 0,
    tags              TEXT NOT NULL DEFAULT '',  -- space-joined, FTS projection
    insights          TEXT NOT NULL DEFAULT '',  -- privacy-safe FTS projection
    intent_json       TEXT NOT NULL,             -- the node with `children` emptied
    source_hash       TEXT NOT NULL,
    created_at        INTEGER NOT NULL
);
CREATE INDEX idx_intents_date ON intents(date, ord);
CREATE INDEX idx_intents_parent ON intents(parent_id);
CREATE INDEX idx_intents_project ON intents(project, date);
CREATE INDEX idx_intents_depth_date ON intents(depth, date);

-- Full-text search over the safe projection; external content = intents.
CREATE VIRTUAL TABLE intent_fts USING fts5(
    label, summary, tags, insights,
    content = 'intents', content_rowid = 'pk',
    tokenize = 'porter unicode61 remove_diacritics 2'
);
CREATE TRIGGER intents_fts_ai AFTER INSERT ON intents BEGIN
    INSERT INTO intent_fts(rowid, label, summary, tags, insights)
    VALUES (new.pk, new.label, new.summary, new.tags, new.insights);
END;
CREATE TRIGGER intents_fts_ad AFTER DELETE ON intents BEGIN
    INSERT INTO intent_fts(intent_fts, rowid, label, summary, tags, insights)
    VALUES ('delete', old.pk, old.label, old.summary, old.tags, old.insights);
END;
CREATE TRIGGER intents_fts_au AFTER UPDATE OF label, summary, tags, insights ON intents BEGIN
    INSERT INTO intent_fts(intent_fts, rowid, label, summary, tags, insights)
    VALUES ('delete', old.pk, old.label, old.summary, old.tags, old.insights);
    INSERT INTO intent_fts(rowid, label, summary, tags, insights)
    VALUES (new.pk, new.label, new.summary, new.tags, new.insights);
END;

-- Identity + dimension of the embedder that produced intent_vec rows (single row).
CREATE TABLE embeddings_meta (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    identity    TEXT NOT NULL,
    dims        INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

-- Small user-facts memory (replaces mem0).
CREATE TABLE facts (
    id                INTEGER PRIMARY KEY,
    subject           TEXT NOT NULL COLLATE NOCASE,
    predicate         TEXT NOT NULL COLLATE NOCASE,
    object            TEXT NOT NULL COLLATE NOCASE,
    source_intent_id  TEXT,
    first_seen_ts     INTEGER NOT NULL,
    last_seen_ts      INTEGER NOT NULL,
    confidence        REAL NOT NULL DEFAULT 1.0,
    embedding         BLOB,
    UNIQUE (subject, predicate, object)
);
CREATE INDEX idx_facts_last_seen ON facts(last_seen_ts);

CREATE TABLE metadata (
    key    TEXT PRIMARY KEY,
    value  TEXT NOT NULL
);

-- Named resumable positions over the (ts, ingested_at, id) event order.
CREATE TABLE ingest_cursors (
    name        TEXT PRIMARY KEY,
    after_ts    INTEGER NOT NULL,
    after_id    TEXT NOT NULL,
    updated_at  INTEGER NOT NULL
);
"#;

/// Apply every migration newer than the recorded version. Returns the final version.
pub(crate) fn migrate(conn: &mut Connection) -> Result<i64> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
             version     INTEGER PRIMARY KEY,
             applied_at  INTEGER NOT NULL
         );",
    )?;
    let mut current: i64 = conn
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| r.get::<_, Option<i64>>(0))
        .optional()?
        .flatten()
        .unwrap_or(0);
    for (version, sql) in MIGRATIONS {
        if *version <= current {
            continue;
        }
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute_batch(sql)?;
        tx.execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
            params![version, now_ts()],
        )?;
        tx.commit()?;
        current = *version;
    }
    Ok(current)
}

/// Latest schema version this build knows about.
pub const SCHEMA_VERSION: i64 = MIGRATIONS[MIGRATIONS.len() - 1].0;
