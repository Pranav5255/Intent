//! Forgetting: delete a day or a project for good.
//!
//! The app promises that forgotten data is deleted from the computer. So a forget
//! removes the raw events too (otherwise the next pipeline run rebuilds the intents),
//! and it ends with [`Store::scrub`], which pushes the deleted pages out of the WAL.
//! `secure_delete` (set on the writer) and FTS5 `secure-delete` (migration 2) zero
//! the deleted bytes inside the database file.

use std::collections::BTreeSet;

use intent_types::RawEvent;
use rusqlite::Transaction;

use crate::Store;
use crate::day::{now_ts, validate_date};
use crate::error::{Result, invalid};
use crate::events::{event_from_row, recompute_day};
use crate::intents::{DeleteReport, collect_id_pk, delete_pks, delete_subtrees, normalize_project};

/// Delete every intent of `date` with its vectors, search rows, facts and pipeline
/// cache. Returns the deleted ids.
pub(crate) fn drop_date_intents(tx: &Transaction<'_>, date: &str) -> Result<Vec<String>> {
    let rows = collect_id_pk(tx, "SELECT id, pk, date FROM intents WHERE date = ?1 ORDER BY id", [date])?;
    let pks: Vec<i64> = rows.iter().map(|r| r.1).collect();
    delete_pks(tx, &pks)?;
    let ids: Vec<String> = rows.into_iter().map(|r| r.0).collect();
    drop_facts(tx, &ids)?;
    tx.execute("DELETE FROM pipeline_runs WHERE date = ?1", [date])?;
    Ok(ids)
}

/// Facts learned from deleted intents go with them.
fn drop_facts(tx: &Transaction<'_>, ids: &[String]) -> Result<()> {
    let mut del = tx.prepare_cached("DELETE FROM facts WHERE source_intent_id = ?1")?;
    for id in ids {
        del.execute([id])?;
    }
    Ok(())
}

impl Store {
    /// Forget everything about `date`: raw events, day stats, intents with their
    /// search rows and vectors, and the pipeline cache, in one transaction. Then
    /// scrub the files.
    pub fn forget_date(&self, date: &str) -> Result<DeleteReport> {
        validate_date(date)?;
        let report = self.write(|tx| {
            let ids = drop_date_intents(tx, date)?;
            let events = tx.execute("DELETE FROM events WHERE day = ?1", [date])?;
            tx.execute("DELETE FROM day_stats WHERE date = ?1", [date])?;
            Ok(DeleteReport {
                deleted_count: ids.len(),
                deleted_intent_ids: ids,
                deleted_event_count: events,
                affected_dates: vec![date.to_string()],
            })
        })?;
        self.scrub()?;
        Ok(report)
    }

    /// Forget a project (`name` or `project:name`) in one transaction:
    ///
    /// * delete `event_ids`, the raw events the engine attributes to the project
    ///   (the store cannot tell; the caller asks the engine);
    /// * delete every intent tagged with the project and its descendants;
    /// * delete *all* intents of every affected date. A session that mixes projects
    ///   keeps the forgotten one in its tags, evidence and resume payload, so the
    ///   caller rebuilds those dates from the events that are left.
    ///
    /// `deleted_intent_ids` lists the project's own intents; `affected_dates` lists
    /// the dates to rebuild.
    pub fn forget_project(&self, project: &str, event_ids: &[String]) -> Result<DeleteReport> {
        let name = normalize_project(project).to_string();
        if name.is_empty() {
            return Err(invalid("project must not be empty"));
        }
        let now = now_ts();
        let report = self.write(|tx| {
            let mut dates: BTreeSet<String> = BTreeSet::new();
            let mut events = 0;
            {
                let mut del = tx.prepare_cached("DELETE FROM events WHERE id = ?1 RETURNING day")?;
                for id in event_ids {
                    let mut rows = del.query([id])?;
                    if let Some(row) = rows.next()? {
                        dates.insert(row.get(0)?);
                        events += 1;
                    }
                }
            }
            let forgotten = delete_subtrees(tx, "SELECT id FROM intents WHERE project = ?1", &name)?;
            let ids: Vec<String> = forgotten.iter().map(|(id, _)| id.clone()).collect();
            drop_facts(tx, &ids)?;
            dates.extend(forgotten.into_iter().map(|(_, date)| date));
            // Parents carry their children's tags; a substring match may also catch a
            // longer name, which only rebuilds one more day.
            let tag = format!("project:{name}");
            let mut stmt = tx.prepare("SELECT DISTINCT date FROM intents WHERE instr(tags, ?1) > 0")?;
            for date in stmt.query_map([&tag], |r| r.get::<_, String>(0))? {
                dates.insert(date?);
            }
            for date in &dates {
                drop_date_intents(tx, date)?;
                recompute_day(tx, date, now)?;
            }
            Ok(DeleteReport {
                deleted_count: ids.len(),
                deleted_intent_ids: ids,
                deleted_event_count: events,
                affected_dates: dates.into_iter().collect(),
            })
        })?;
        self.scrub()?;
        Ok(report)
    }

    /// Events whose JSON payload contains `text`, oldest first. This scans every
    /// event, so callers use it only for rare user actions and filter the hits.
    pub fn events_mentioning(&self, text: &str) -> Result<Vec<RawEvent>> {
        if text.is_empty() {
            return Err(invalid("text must not be empty"));
        }
        self.read(|c| {
            let mut stmt = c.prepare(
                "SELECT id, schema_version, ts, source, type, payload, ingested_at FROM events
                 WHERE instr(payload, ?1) > 0 ORDER BY ts, ingested_at, id",
            )?;
            let rows = stmt.query_map([text], event_from_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Copy the WAL into the database and truncate it, so deleted rows do not
    /// survive in old WAL frames. A reader that holds a snapshot past the busy
    /// timeout leaves the WAL in place; the next scrub or checkpoint clears it.
    pub fn scrub(&self) -> Result<()> {
        let conn = self.writer_conn();
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(())
    }
}
