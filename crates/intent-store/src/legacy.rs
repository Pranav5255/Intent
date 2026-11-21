//! One-time importer for the 1.x `events.db` (capture server) and `intents.db`
//! (engine). Each source is imported at most once (recorded in `metadata`), and every
//! row insert is idempotent, so re-running after a crash is safe.

use std::path::Path;

use intent_types::{Intent, Payload, RawEvent};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::Store;
use crate::day::now_ts;
use crate::error::Result;
use crate::events::insert_tx;
use crate::intents::insert_node;

const EVENTS_FLAG: &str = "legacy_import.events";
const INTENTS_FLAG: &str = "legacy_import.intents";
const CHUNK: usize = 5_000;

/// What [`Store::import_legacy`] did.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ImportReport {
    pub events_db_found: bool,
    pub intents_db_found: bool,
    /// Sources skipped because a previous import already completed.
    pub events_already_imported: bool,
    pub intents_already_imported: bool,
    pub events_imported: usize,
    pub events_skipped: usize,
    pub intents_imported: usize,
    pub intents_skipped: usize,
    pub runs_imported: usize,
}

fn open_legacy(path: &Path) -> Result<Option<Connection>> {
    if !path.is_file() {
        return Ok(None);
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(Some(conn))
}

fn has_table(conn: &Connection, name: &str) -> Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1", [name], |_| Ok(()))
        .optional()?
        .is_some())
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = stmt.query_map([], |r| r.get::<_, String>(1))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(names.iter().any(|n| n == column))
}

fn flag_set(tx: &Transaction<'_>, key: &str) -> Result<bool> {
    Ok(tx.query_row("SELECT 1 FROM metadata WHERE key = ?1", [key], |_| Ok(())).optional()?.is_some())
}

fn set_flag(tx: &Transaction<'_>, key: &str, report: &ImportReport) -> Result<()> {
    let value = serde_json::json!({ "at": now_ts(), "report": report }).to_string();
    tx.execute(
        "INSERT INTO metadata(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

fn import_events(tx: &Transaction<'_>, src: &Connection, store: &Store, report: &mut ImportReport) -> Result<()> {
    if !has_table(src, "events")? {
        return Ok(());
    }
    let sv = if has_column(src, "events", "schema_version")? { "schema_version" } else { "1" };
    let mut stmt = src.prepare(&format!(
        "SELECT id, {sv}, ts, source, type, payload, ingested_at FROM events ORDER BY ts, ingested_at, id"
    ))?;
    let mut rows = stmt.query([])?;
    let zone = store.day_zone();
    let now = now_ts();
    let mut batch: Vec<RawEvent> = Vec::with_capacity(CHUNK);
    let flush = |batch: &mut Vec<RawEvent>, report: &mut ImportReport| -> Result<()> {
        let r = insert_tx(tx, &zone, batch.iter(), now)?;
        report.events_imported += r.inserted_ids().count();
        batch.clear();
        Ok(())
    };
    while let Some(row) = rows.next()? {
        let payload_text: String = row.get(5)?;
        let Ok(payload) = serde_json::from_str::<Payload>(&payload_text) else {
            report.events_skipped += 1;
            continue;
        };
        batch.push(RawEvent {
            id: row.get(0)?,
            schema_version: row.get(1)?,
            ts: row.get(2)?,
            source: row.get(3)?,
            kind: row.get(4)?,
            payload,
            ingested_at: row.get(6)?,
        });
        if batch.len() >= CHUNK {
            flush(&mut batch, report)?;
        }
    }
    flush(&mut batch, report)?;
    Ok(())
}

fn import_intents(tx: &Transaction<'_>, src: &Connection, report: &mut ImportReport) -> Result<()> {
    if has_table(src, "intents")? {
        let mut stmt = src.prepare(
            "SELECT id, date, parent_id, intent_json, COALESCE(source_hash, ''), COALESCE(created_at, 0)
             FROM intents ORDER BY date, start_ts, id",
        )?;
        let mut rows = stmt.query([])?;
        let mut ord: i64 = 0;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let date: String = row.get(1)?;
            let parent_id: Option<String> = row.get(2)?;
            let json: String = row.get(3)?;
            let source_hash: String = row.get(4)?;
            let created_at: i64 = row.get(5)?;
            let exists = tx
                .prepare_cached("SELECT 1 FROM intents WHERE id = ?1")?
                .query_row([&id], |_| Ok(()))
                .optional()?
                .is_some();
            if exists {
                continue;
            }
            let Ok(mut node) = serde_json::from_str::<Intent>(&json) else {
                report.intents_skipped += 1;
                continue;
            };
            // 1.x stored nodes without children; enforce it and trust the row columns.
            node.children.clear();
            node.parent_id = parent_id;
            node.id = id;
            insert_node(tx, &date, &node, ord, &source_hash, if created_at > 0 { created_at } else { now_ts() })?;
            ord += 1;
            report.intents_imported += 1;
        }
    }
    if has_table(src, "pipeline_runs")? {
        let mut stmt = src.prepare(
            "SELECT date, source_hash, COALESCE(status, 'complete'), COALESCE(warnings_json, '[]'),
                    COALESCE(started_at, 0), completed_at
             FROM pipeline_runs WHERE date IS NOT NULL AND source_hash IS NOT NULL",
        )?;
        let mut rows = stmt.query([])?;
        let mut insert = tx.prepare_cached(
            "INSERT INTO pipeline_runs(date, source_hash, status, warnings_json, started_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(date, source_hash) DO NOTHING",
        )?;
        while let Some(row) = rows.next()? {
            let n = insert.execute(params![
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, Option<i64>>(5)?,
            ])?;
            report.runs_imported += n;
        }
    }
    Ok(())
}

impl Store {
    /// Copy 1.x data into this store if the legacy files exist. Each file is
    /// imported in one transaction and flagged in `metadata` so later calls are
    /// no-ops. Events are bucketed with the store's current [`crate::DayZone`].
    pub fn import_legacy(&self, events_db: &Path, intents_db: &Path) -> Result<ImportReport> {
        let mut report = ImportReport::default();
        if let Some(src) = open_legacy(events_db)? {
            report.events_db_found = true;
            self.write(|tx| {
                if flag_set(tx, EVENTS_FLAG)? {
                    report.events_already_imported = true;
                    return Ok(());
                }
                import_events(tx, &src, self, &mut report)?;
                set_flag(tx, EVENTS_FLAG, &report)
            })?;
        }
        if let Some(src) = open_legacy(intents_db)? {
            report.intents_db_found = true;
            self.write(|tx| {
                if flag_set(tx, INTENTS_FLAG)? {
                    report.intents_already_imported = true;
                    return Ok(());
                }
                import_intents(tx, &src, &mut report)?;
                set_flag(tx, INTENTS_FLAG, &report)
            })?;
        }
        Ok(report)
    }
}
