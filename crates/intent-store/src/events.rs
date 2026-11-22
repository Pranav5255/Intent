//! Raw capture events: idempotent ingest, day stats, export/pagination, retention.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use intent_types::{DayExport, DayExportMeta, EventCursor, Payload, RawEvent};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::Store;
use crate::day::{DayZone, now_ts, validate_date};
use crate::error::{Result, invalid};
use crate::forget::drop_date_intents;

/// `(source, type)` pairs that carry content-level ("detailed") capture.
/// This is the ONE list used for retention tiers, counts and purges.
pub const DETAILED_EVENT_KINDS: &[(&str, &str)] = &[
    ("vscode", "document_change"),
    ("firefox", "user_action"),
    ("chrome", "user_action"),
    ("filesystem", "file_content"),
];

/// Whether an event of this `source`/`type` belongs to the detailed tier.
pub fn is_detailed(source: &str, kind: &str) -> bool {
    DETAILED_EVENT_KINDS.iter().any(|(s, t)| *s == source && *t == kind)
}

/// SQL predicate matching [`DETAILED_EVENT_KINDS`]; built from constants only.
fn detailed_clause() -> String {
    let parts: Vec<String> = DETAILED_EVENT_KINDS
        .iter()
        .map(|(s, t)| format!("(source = '{}' AND type = '{}')", s.replace('\'', "''"), t.replace('\'', "''")))
        .collect();
    format!("({})", parts.join(" OR "))
}

const EVENT_COLUMNS: &str = "id, schema_version, ts, source, type, payload, ingested_at";
const EVENT_ORDER: &str = "ORDER BY ts ASC, ingested_at ASC, id ASC";

/// Result of inserting one event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsertOutcome {
    /// `false` when the id already existed (the stored copy is returned in `event`).
    pub inserted: bool,
    pub event: RawEvent,
}

/// Day-stat change caused by one insert batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DayStatsDelta {
    pub date: String,
    pub inserted: i64,
    pub event_count: i64,
    pub revision: i64,
}

/// Result of [`Store::insert_events`]; `outcomes` is parallel to the input batch.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InsertReport {
    pub outcomes: Vec<InsertOutcome>,
    pub days: Vec<DayStatsDelta>,
}

impl InsertReport {
    /// Ids that were newly inserted (duplicates excluded).
    pub fn inserted_ids(&self) -> impl Iterator<Item = &str> {
        self.outcomes.iter().filter(|o| o.inserted).map(|o| o.event.id.as_str())
    }
}

/// One page of `events_page`, read from a single snapshot together with the day meta.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventPage {
    pub date: String,
    pub events: Vec<RawEvent>,
    pub has_more: bool,
    pub next_cursor: Option<EventCursor>,
    pub revision: i64,
    pub content_hash: String,
}

/// Per-source health numbers for `/v1/status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceStats {
    pub source: String,
    pub event_count: i64,
    pub last_event_ts: Option<i64>,
    pub last_ingested_at: Option<i64>,
}

/// Absolute retention cutoffs (unix seconds). Events strictly older are eligible.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionCutoffs {
    /// Detailed-tier cutoff. The effective detailed cutoff is the later of this and
    /// `metadata_before` (detailed data never outlives metadata).
    pub detailed_before: Option<i64>,
    /// Cutoff for everything else.
    pub metadata_before: Option<i64>,
}

impl RetentionCutoffs {
    /// Build cutoffs from "keep N days" settings (`None` = keep forever).
    pub fn from_days(detailed_days: Option<u32>, metadata_days: Option<u32>, now: i64) -> Self {
        Self {
            detailed_before: detailed_days.map(|d| now - i64::from(d) * 86_400),
            metadata_before: metadata_days.map(|d| now - i64::from(d) * 86_400),
        }
    }

    fn effective_detailed(&self) -> Option<i64> {
        match (self.detailed_before, self.metadata_before) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionCounts {
    pub detailed: i64,
    pub metadata: i64,
    /// Total events (`detailed + metadata`).
    pub total: i64,
    /// Intent rows (roots and children) of days past the metadata cutoff.
    #[serde(default)]
    pub intents: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PurgeReport {
    pub deleted: RetentionCounts,
    /// Days whose stats were recomputed.
    pub affected_days: Vec<String>,
}

fn content_hash(revision: i64, count: i64, last_id: Option<&str>) -> String {
    format!("rev:{revision}|count:{count}|last:{}", last_id.unwrap_or(""))
}

pub(crate) fn event_from_row(row: &Row<'_>) -> rusqlite::Result<RawEvent> {
    let payload_text: String = row.get(5)?;
    let payload: Payload = serde_json::from_str(&payload_text)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(e)))?;
    Ok(RawEvent {
        id: row.get(0)?,
        schema_version: row.get(1)?,
        ts: row.get(2)?,
        source: row.get(3)?,
        kind: row.get(4)?,
        payload,
        ingested_at: Some(row.get(6)?),
    })
}

/// Aggregate of one batch's inserts into one day.
struct DayAgg {
    n: i64,
    first_ts: i64,
    /// Max of (ts, ingested_at, id) — the export order's last element.
    last: (i64, i64, String),
}

/// Insert `events` inside an open write transaction. Shared by the public API and the
/// legacy importer. Day stats are updated once per touched day.
pub(crate) fn insert_tx<'a>(
    tx: &Transaction<'_>,
    zone: &DayZone,
    events: impl IntoIterator<Item = &'a RawEvent>,
    now: i64,
) -> Result<InsertReport> {
    let mut outcomes = Vec::new();
    let mut days: HashMap<String, DayAgg> = HashMap::new();
    {
        let mut insert = tx.prepare_cached(
            "INSERT INTO events(id, schema_version, ts, source, type, payload, ingested_at, day)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) ON CONFLICT(id) DO NOTHING",
        )?;
        let mut existing = tx.prepare_cached(&format!("SELECT {EVENT_COLUMNS} FROM events WHERE id = ?1"))?;
        for ev in events {
            let ingested_at = ev.ingested_at.unwrap_or(now);
            let day = zone.day_for_ts(ev.ts);
            let payload = serde_json::to_string(&ev.payload)?;
            let n = insert.execute(params![
                ev.id,
                ev.schema_version,
                ev.ts,
                ev.source,
                ev.kind,
                payload,
                ingested_at,
                day
            ])?;
            if n == 0 {
                let stored = existing.query_row([&ev.id], event_from_row)?;
                outcomes.push(InsertOutcome { inserted: false, event: stored });
                continue;
            }
            let key = (ev.ts, ingested_at, ev.id.clone());
            days.entry(day)
                .and_modify(|a| {
                    a.n += 1;
                    a.first_ts = a.first_ts.min(ev.ts);
                    if key > a.last {
                        a.last = key.clone();
                    }
                })
                .or_insert_with(|| DayAgg { n: 1, first_ts: ev.ts, last: key.clone() });
            let mut stored = ev.clone();
            stored.ingested_at = Some(ingested_at);
            outcomes.push(InsertOutcome { inserted: true, event: stored });
        }
    }

    let mut deltas = Vec::with_capacity(days.len());
    let mut read = tx.prepare_cached(
        "SELECT event_count, first_ts, last_ts, last_ingested_at, last_event_id, revision FROM day_stats WHERE date = ?1",
    )?;
    let mut upsert = tx.prepare_cached(
        "INSERT INTO day_stats(date, event_count, first_ts, last_ts, last_event_id, last_ingested_at, revision, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(date) DO UPDATE SET
             event_count = excluded.event_count, first_ts = excluded.first_ts,
             last_ts = excluded.last_ts, last_event_id = excluded.last_event_id,
             last_ingested_at = excluded.last_ingested_at, revision = excluded.revision,
             updated_at = excluded.updated_at",
    )?;
    let mut sorted: Vec<_> = days.into_iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    for (date, agg) in sorted {
        type Prev = (i64, Option<i64>, Option<i64>, Option<i64>, Option<String>, i64);
        let prev: Option<Prev> = read
            .query_row([&date], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
            .optional()?;
        let (count, first_ts, last, revision) = match prev {
            None => (agg.n, agg.first_ts, agg.last, agg.n),
            Some((count, first, lts, ling, lid, rev)) => {
                let first = first.map_or(agg.first_ts, |f| f.min(agg.first_ts));
                let prev_last = match (lts, lid) {
                    (Some(t), Some(id)) => Some((t, ling.unwrap_or(i64::MIN), id)),
                    _ => None,
                };
                let last = match prev_last {
                    Some(p) if p > agg.last => p,
                    _ => agg.last,
                };
                // Like 1.x, revision advances once per inserted event.
                (count + agg.n, first, last, rev + agg.n)
            }
        };
        upsert.execute(params![date, count, first_ts, last.0, last.2, last.1, revision, now])?;
        deltas.push(DayStatsDelta { date, inserted: agg.n, event_count: count, revision });
    }
    Ok(InsertReport { outcomes, days: deltas })
}

/// Recompute one day's stats from `events` (uses `idx_events_day`).
pub(crate) fn recompute_day(tx: &Connection, date: &str, now: i64) -> Result<()> {
    let (count, first_ts): (i64, Option<i64>) = tx
        .prepare_cached("SELECT COUNT(*), MIN(ts) FROM events WHERE day = ?1")?
        .query_row([date], |r| Ok((r.get(0)?, r.get(1)?)))?;
    if count == 0 {
        tx.prepare_cached("DELETE FROM day_stats WHERE date = ?1")?.execute([date])?;
        return Ok(());
    }
    let (last_id, last_ts, last_ing): (String, i64, i64) = tx
        .prepare_cached(
            "SELECT id, ts, ingested_at FROM events WHERE day = ?1
             ORDER BY ts DESC, ingested_at DESC, id DESC LIMIT 1",
        )?
        .query_row([date], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    tx.prepare_cached(
        "INSERT INTO day_stats(date, event_count, first_ts, last_ts, last_event_id, last_ingested_at, revision, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7)
         ON CONFLICT(date) DO UPDATE SET
             event_count = excluded.event_count, first_ts = excluded.first_ts,
             last_ts = excluded.last_ts, last_event_id = excluded.last_event_id,
             last_ingested_at = excluded.last_ingested_at, revision = day_stats.revision + 1,
             updated_at = excluded.updated_at",
    )?
    .execute(params![date, count, first_ts, last_ts, last_id, last_ing, now])?;
    Ok(())
}

/// `DELETE ... RETURNING day`, collecting the touched days. Returns the row count.
fn delete_collecting_days(tx: &Connection, sql: &str, cutoff: i64, days: &mut BTreeSet<String>) -> Result<i64> {
    let mut stmt = tx.prepare_cached(sql)?;
    let mut rows = stmt.query([cutoff])?;
    let mut n = 0;
    while let Some(row) = rows.next()? {
        days.insert(row.get(0)?);
        n += 1;
    }
    Ok(n)
}

/// `(event_count, first_ts, last_ts, last_event_id, revision)` from `day_stats`.
type DayStatsRow = (i64, Option<i64>, Option<i64>, Option<String>, i64);

fn day_meta_on(conn: &Connection, date: &str) -> Result<DayExportMeta> {
    let row: Option<DayStatsRow> = conn
        .prepare_cached(
            "SELECT event_count, first_ts, last_ts, last_event_id, revision FROM day_stats WHERE date = ?1",
        )?
        .query_row([date], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
        .optional()?;
    let checked_at = now_ts();
    Ok(match row {
        None => DayExportMeta {
            version: 1,
            date: date.to_string(),
            event_count: 0,
            first_ts: None,
            last_ts: None,
            last_event_id: None,
            revision: 0,
            content_hash: content_hash(0, 0, None),
            checked_at,
        },
        Some((count, first_ts, last_ts, last_event_id, revision)) => DayExportMeta {
            version: 1,
            date: date.to_string(),
            event_count: count,
            first_ts,
            last_ts,
            content_hash: content_hash(revision, count, last_event_id.as_deref()),
            last_event_id,
            revision,
            checked_at,
        },
    })
}

fn query_events(conn: &Connection, sql: &str, params: impl rusqlite::Params) -> Result<Vec<RawEvent>> {
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map(params, event_from_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

impl Store {
    /// Insert a batch idempotently (`ON CONFLICT(id) DO NOTHING`) and update the
    /// affected day stats, all in ONE transaction. `ingested_at` defaults to now.
    pub fn insert_events(&self, batch: &[RawEvent]) -> Result<InsertReport> {
        if batch.is_empty() {
            return Ok(InsertReport::default());
        }
        let zone = self.day_zone();
        let now = now_ts();
        self.write(|tx| insert_tx(tx, &zone, batch, now))
    }

    /// Convenience wrapper over [`Store::insert_events`] for one event.
    pub fn insert_event(&self, event: &RawEvent) -> Result<InsertOutcome> {
        let mut report = self.insert_events(std::slice::from_ref(event))?;
        Ok(report.outcomes.remove(0))
    }

    /// All events bucketed into `date`, in export order `(ts, ingested_at, id)`.
    pub fn events_for_day(&self, date: &str) -> Result<Vec<RawEvent>> {
        validate_date(date)?;
        self.read(|c| {
            query_events(c, &format!("SELECT {EVENT_COLUMNS} FROM events WHERE day = ?1 {EVENT_ORDER}"), [date])
        })
    }

    /// Events with `ts >= since`, in export order.
    pub fn events_since(&self, since: i64) -> Result<Vec<RawEvent>> {
        self.read(|c| {
            query_events(c, &format!("SELECT {EVENT_COLUMNS} FROM events WHERE ts >= ?1 {EVENT_ORDER}"), [since])
        })
    }

    /// `/v1/export/day` payload (events + meta read from one snapshot).
    pub fn export_day(&self, date: &str) -> Result<DayExport> {
        validate_date(date)?;
        let events = self.events_for_day(date)?;
        Ok(DayExport { version: 1, date: date.to_string(), exported_at: now_ts(), events })
    }

    /// Keyset-paginated events for one day, ordered `(ts, ingested_at, id)`.
    ///
    /// The wire cursor only carries `(after_ts, after_id)`; the cursor event's
    /// `ingested_at` is looked up so the keyset matches the sort order exactly (1.x
    /// compared `(ts, id)` against a `(ts, ingested_at, id)` sort and could skip rows).
    /// If the cursor event has since been purged, every event at `after_ts` is
    /// conservatively re-delivered rather than risking a skip (clients dedupe by id).
    pub fn events_page(&self, date: &str, after: Option<&EventCursor>, limit: usize) -> Result<EventPage> {
        validate_date(date)?;
        if !(1..=2000).contains(&limit) {
            return Err(invalid("limit must be between 1 and 2000"));
        }
        let fetch = (limit + 1) as i64;
        self.read(|c| {
            let meta = day_meta_on(c, date)?;
            let mut events = match after {
                None => query_events(
                    c,
                    &format!("SELECT {EVENT_COLUMNS} FROM events WHERE day = ?1 {EVENT_ORDER} LIMIT ?2"),
                    params![date, fetch],
                )?,
                Some(cur) => {
                    let ing: i64 = c
                        .prepare_cached("SELECT ingested_at FROM events WHERE id = ?1 AND ts = ?2")?
                        .query_row(params![cur.after_id, cur.after_ts], |r| r.get(0))
                        .optional()?
                        .unwrap_or(i64::MIN);
                    query_events(
                        c,
                        &format!(
                            "SELECT {EVENT_COLUMNS} FROM events
                             WHERE day = ?1 AND (ts, ingested_at, id) > (?2, ?3, ?4) {EVENT_ORDER} LIMIT ?5"
                        ),
                        params![date, cur.after_ts, ing, cur.after_id, fetch],
                    )?
                }
            };
            let has_more = events.len() > limit;
            events.truncate(limit);
            let next_cursor = if has_more {
                events.last().map(|e| EventCursor { after_ts: e.ts, after_id: e.id.clone() })
            } else {
                None
            };
            Ok(EventPage {
                date: date.to_string(),
                events,
                has_more,
                next_cursor,
                revision: meta.revision,
                content_hash: meta.content_hash,
            })
        })
    }

    /// `/v1/export/day/meta`.
    pub fn day_meta(&self, date: &str) -> Result<DayExportMeta> {
        validate_date(date)?;
        self.read(|c| day_meta_on(c, date))
    }

    /// Count of detailed-tier events keyed `source/type` (only non-zero kinds).
    pub fn detailed_event_counts(&self) -> Result<BTreeMap<String, i64>> {
        self.read(|c| {
            let sql = format!(
                "SELECT source || '/' || type, COUNT(*) FROM events WHERE {} GROUP BY source, type",
                detailed_clause()
            );
            let mut stmt = c.prepare_cached(&sql)?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<BTreeMap<_, _>>>()?)
        })
    }

    /// Per-source count / last ts / last ingest, one GROUP BY over a covering index.
    pub fn source_stats(&self) -> Result<Vec<SourceStats>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT source, COUNT(*), MAX(ts), MAX(ingested_at) FROM events GROUP BY source ORDER BY source",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(SourceStats {
                    source: r.get(0)?,
                    event_count: r.get(1)?,
                    last_event_ts: r.get(2)?,
                    last_ingested_at: r.get(3)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// How many events each tier, and how many intents, a purge would delete.
    pub fn retention_preview(&self, cutoffs: &RetentionCutoffs) -> Result<RetentionCounts> {
        let clause = detailed_clause();
        let intent_cutoff = self.intent_cutoff(cutoffs);
        self.read(|c| {
            let detailed = match cutoffs.effective_detailed() {
                Some(cut) => c
                    .prepare_cached(&format!("SELECT COUNT(*) FROM events WHERE {clause} AND ts < ?1"))?
                    .query_row([cut], |r| r.get(0))?,
                None => 0,
            };
            let metadata = match cutoffs.metadata_before {
                Some(cut) => c
                    .prepare_cached(&format!("SELECT COUNT(*) FROM events WHERE NOT {clause} AND ts < ?1"))?
                    .query_row([cut], |r| r.get(0))?,
                None => 0,
            };
            let intents = match &intent_cutoff {
                Some(day) => {
                    c.prepare_cached("SELECT COUNT(*) FROM intents WHERE date < ?1")?.query_row([day], |r| r.get(0))?
                }
                None => 0,
            };
            Ok(RetentionCounts { detailed, metadata, total: detailed + metadata, intents })
        })
    }

    /// Delete events past the retention cutoffs and recompute stats for only the
    /// days that actually lost rows (collected via `DELETE ... RETURNING day`).
    /// Intents of whole days before the metadata cutoff go too, with their search
    /// rows and vectors.
    pub fn purge_retention(&self, cutoffs: &RetentionCutoffs) -> Result<PurgeReport> {
        let clause = detailed_clause();
        let now = now_ts();
        let intent_cutoff = self.intent_cutoff(cutoffs);
        let report = self.write(|tx| {
            let mut days = BTreeSet::new();
            let detailed = match cutoffs.effective_detailed() {
                Some(cut) => delete_collecting_days(
                    tx,
                    &format!("DELETE FROM events WHERE {clause} AND ts < ?1 RETURNING day"),
                    cut,
                    &mut days,
                )?,
                None => 0,
            };
            let metadata = match cutoffs.metadata_before {
                Some(cut) => delete_collecting_days(
                    tx,
                    &format!("DELETE FROM events WHERE NOT {clause} AND ts < ?1 RETURNING day"),
                    cut,
                    &mut days,
                )?,
                None => 0,
            };
            for day in &days {
                recompute_day(tx, day, now)?;
            }
            let mut intents = 0;
            if let Some(cutoff) = &intent_cutoff {
                let dates: Vec<String> = {
                    let mut stmt = tx.prepare_cached("SELECT DISTINCT date FROM intents WHERE date < ?1")?;
                    stmt.query_map([cutoff], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
                };
                for date in &dates {
                    intents += drop_date_intents(tx, date)?.len() as i64;
                }
                tx.execute("DELETE FROM pipeline_runs WHERE date < ?1", [cutoff])?;
            }
            Ok(PurgeReport {
                deleted: RetentionCounts { detailed, metadata, total: detailed + metadata, intents },
                affected_days: days.into_iter().collect(),
            })
        })?;
        if report.deleted.total > 0 || report.deleted.intents > 0 {
            self.scrub()?;
        }
        Ok(report)
    }

    /// Intents of days before this date are past the metadata cutoff. Whole days
    /// only: every event of such a day is older than the cutoff too.
    fn intent_cutoff(&self, cutoffs: &RetentionCutoffs) -> Option<String> {
        cutoffs.metadata_before.map(|cut| self.local_day(cut))
    }

    /// Delete every detailed-tier event regardless of age (consent withdrawn).
    pub fn purge_detailed_events(&self) -> Result<PurgeReport> {
        let clause = detailed_clause();
        let now = now_ts();
        let report = self.write(|tx| {
            let mut days = BTreeSet::new();
            let detailed = delete_collecting_days(
                tx,
                &format!("DELETE FROM events WHERE {clause} AND ts < ?1 RETURNING day"),
                i64::MAX,
                &mut days,
            )?;
            for day in &days {
                recompute_day(tx, day, now)?;
            }
            Ok(PurgeReport {
                deleted: RetentionCounts { detailed, metadata: 0, total: detailed, intents: 0 },
                affected_days: days.into_iter().collect(),
            })
        })?;
        if report.deleted.total > 0 {
            self.scrub()?;
        }
        Ok(report)
    }

    /// Recompute every event's `day` with the current [`DayZone`] and rebuild all day
    /// stats. Use after the user changes their time zone setting. Returns rows moved.
    pub fn rebucket_days(&self) -> Result<usize> {
        let now = now_ts();
        self.write(|tx| {
            let moved = tx.execute("UPDATE events SET day = intent_day(ts) WHERE day <> intent_day(ts)", [])?;
            tx.execute("DELETE FROM day_stats", [])?;
            let days: Vec<String> = {
                let mut stmt = tx.prepare("SELECT DISTINCT day FROM events")?;
                stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
            };
            for day in &days {
                recompute_day(tx, day, now)?;
            }
            Ok(moved)
        })
    }
}
