//! Key/value metadata and named ingest cursors.

use intent_types::EventCursor;
use rusqlite::{OptionalExtension, params};

use crate::Store;
use crate::day::now_ts;
use crate::error::Result;

impl Store {
    /// Read one metadata value.
    pub fn get_metadata(&self, key: &str) -> Result<Option<String>> {
        self.read(|c| {
            Ok(c.prepare_cached("SELECT value FROM metadata WHERE key = ?1")?
                .query_row([key], |r| r.get(0))
                .optional()?)
        })
    }

    /// Write one metadata value.
    pub fn set_metadata(&self, key: &str, value: &str) -> Result<()> {
        self.set_metadata_many(&[(key, value)])
    }

    /// Atomically write several metadata values.
    pub fn set_metadata_many(&self, values: &[(&str, &str)]) -> Result<()> {
        if values.is_empty() {
            return Ok(());
        }
        self.write(|tx| {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO metadata(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            )?;
            for (k, v) in values {
                stmt.execute([k, v])?;
            }
            Ok(())
        })
    }

    /// Remove a metadata key; `true` if it existed.
    pub fn delete_metadata(&self, key: &str) -> Result<bool> {
        self.write(|tx| Ok(tx.execute("DELETE FROM metadata WHERE key = ?1", [key])? > 0))
    }

    /// Position of a named consumer in the `(ts, ingested_at, id)` event stream.
    pub fn ingest_cursor(&self, name: &str) -> Result<Option<EventCursor>> {
        self.read(|c| {
            Ok(c.prepare_cached("SELECT after_ts, after_id FROM ingest_cursors WHERE name = ?1")?
                .query_row([name], |r| Ok(EventCursor { after_ts: r.get(0)?, after_id: r.get(1)? }))
                .optional()?)
        })
    }

    /// Persist a consumer's position.
    pub fn set_ingest_cursor(&self, name: &str, cursor: &EventCursor) -> Result<()> {
        self.write(|tx| {
            tx.prepare_cached(
                "INSERT INTO ingest_cursors(name, after_ts, after_id, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(name) DO UPDATE SET after_ts = excluded.after_ts, after_id = excluded.after_id,
                                                 updated_at = excluded.updated_at",
            )?
            .execute(params![name, cursor.after_ts, cursor.after_id, now_ts()])?;
            Ok(())
        })
    }
}
