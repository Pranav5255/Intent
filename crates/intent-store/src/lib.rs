//! `intent-store`: the single SQLite database behind Intent 2.0.
//!
//! * One file (WAL mode) holding raw events, day stats, intents, pipeline runs,
//!   FTS5 search, sqlite-vec embeddings, user facts and metadata.
//! * [`Store`] is `Clone + Send + Sync`: one writer connection behind a mutex (every
//!   write is a single `BEGIN IMMEDIATE` transaction) plus a small pool of read-only
//!   connections that never block the writer thanks to WAL.
//! * The API is synchronous. Async callers (`intentd`) should wrap calls in
//!   `tokio::task::spawn_blocking`.
//!
//! Module map: [`events`] (capture side), [`intents`] (pipeline results, trees, stats),
//! [`search`] (FTS5, sqlite-vec, hybrid), [`facts`], [`meta`] (metadata + cursors),
//! [`legacy`] (0.1 importer), `forget` (forget a day or a project, scrub the files).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Once, RwLock};
use std::time::Duration;

use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};

pub mod day;
pub mod error;
pub mod events;
pub mod facts;
mod forget;
pub mod intents;
pub mod legacy;
pub mod meta;
mod schema;
pub mod search;

pub use day::{DayZone, now_ts, validate_date};
pub use error::{Result, StoreError};
pub use events::{
    DETAILED_EVENT_KINDS, DayStatsDelta, EventPage, InsertOutcome, InsertReport, PurgeReport, RetentionCounts,
    RetentionCutoffs, SourceStats, is_detailed,
};
pub use facts::{Fact, FactInput, FactQuery};
pub use intents::{CachedRun, DayBucket, DeleteReport, IntentStatsReport, LabelCount, ProjectCount, RootIntent};
pub use legacy::ImportReport;
pub use schema::SCHEMA_VERSION;
pub use search::{EmbedderSpec, EmbeddingTodo, fts_query};

/// Tunables for [`Store::open`].
#[derive(Debug, Clone)]
pub struct StoreOptions {
    /// Zone used to bucket newly inserted events into calendar days.
    pub day_zone: DayZone,
    /// Number of read-only connections in the pool (min 1).
    pub read_connections: usize,
    /// How long a connection waits on a locked database before `SQLITE_BUSY`.
    pub busy_timeout: Duration,
    /// Embedder that will produce intent vectors. `None` keeps whatever is configured
    /// already (or leaves vector search disabled on a fresh DB).
    pub embedder: Option<EmbedderSpec>,
}

impl Default for StoreOptions {
    fn default() -> Self {
        Self { day_zone: DayZone::Local, read_connections: 4, busy_timeout: Duration::from_secs(5), embedder: None }
    }
}

/// Handle to the Intent database. Cheap to clone; share it freely across threads.
#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    writer: Mutex<Connection>,
    readers: Vec<Mutex<Connection>>,
    next_reader: AtomicUsize,
    day_zone: Arc<RwLock<DayZone>>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").field("path", &self.inner.path).finish_non_exhaustive()
    }
}

/// Register sqlite-vec for every connection opened by this process (idempotent).
fn register_sqlite_vec() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: `sqlite3_vec_init` is the extension entry point with the standard
        // `(db, pzErrMsg, pApi)` signature; sqlite-vec's own docs register it this way.
        // It is registered once, before any connection is opened by the store.
        unsafe {
            #[allow(clippy::missing_transmute_annotations)]
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(sqlite_vec::sqlite3_vec_init as *const ())));
        }
    });
}

/// Per-connection pragmas shared by the writer and readers.
fn configure(conn: &Connection, busy: Duration) -> Result<()> {
    conn.busy_timeout(busy)?;
    conn.set_prepared_statement_cache_capacity(64);
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    // Zero deleted rows and freed pages: forgotten data must not stay in the file.
    conn.pragma_update(None, "secure_delete", true)?;
    Ok(())
}

impl Store {
    /// Open (or create) the database at `path`, run migrations and build the pool.
    pub fn open(path: impl AsRef<Path>, opts: StoreOptions) -> Result<Store> {
        register_sqlite_vec();
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let day_zone = Arc::new(RwLock::new(opts.day_zone));

        let mut writer = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        configure(&writer, opts.busy_timeout)?;
        let mode: String = writer.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(StoreError::InvalidArgument(format!("could not enable WAL (journal_mode={mode})")));
        }
        // `intent_day(ts)` lets SQL re-bucket events with the store's current zone.
        let zone_for_fn = Arc::clone(&day_zone);
        writer.create_scalar_function("intent_day", 1, FunctionFlags::SQLITE_UTF8, move |ctx| {
            let ts: i64 = ctx.get(0)?;
            let zone = zone_for_fn.read().unwrap_or_else(|e| e.into_inner()).clone();
            Ok(zone.day_for_ts(ts))
        })?;
        schema::migrate(&mut writer)?;
        {
            let tx = writer.transaction_with_behavior(TransactionBehavior::Immediate)?;
            search::init_embeddings(&tx, opts.embedder.as_ref())?;
            tx.commit()?;
        }

        let n = opts.read_connections.max(1);
        let mut readers = Vec::with_capacity(n);
        for _ in 0..n {
            let conn =
                Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
            conn.busy_timeout(opts.busy_timeout)?;
            conn.set_prepared_statement_cache_capacity(64);
            readers.push(Mutex::new(conn));
        }

        Ok(Store {
            inner: Arc::new(Inner {
                path,
                writer: Mutex::new(writer),
                readers,
                next_reader: AtomicUsize::new(0),
                day_zone,
            }),
        })
    }

    /// Path of the database file.
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// Zone currently used to bucket new events into days.
    pub fn day_zone(&self) -> DayZone {
        self.inner.day_zone.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Change the bucketing zone for future inserts. Existing rows keep their day;
    /// call [`Store::rebucket_days`] to recompute them.
    pub fn set_day_zone(&self, zone: DayZone) {
        *self.inner.day_zone.write().unwrap_or_else(|e| e.into_inner()) = zone;
    }

    /// Calendar day (`YYYY-MM-DD`) of `ts` in the store's configured zone.
    pub fn local_day(&self, ts: i64) -> String {
        self.day_zone().day_for_ts(ts)
    }

    /// Run `f` inside one `BEGIN IMMEDIATE` transaction on the writer connection.
    /// The transaction commits iff `f` returns `Ok`.
    pub(crate) fn write<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let mut conn = lock(&self.inner.writer);
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    /// Direct access to the writer connection outside a transaction (ATTACH, VACUUM...).
    pub(crate) fn writer_conn(&self) -> MutexGuard<'_, Connection> {
        lock(&self.inner.writer)
    }

    /// Run `f` on a pooled read connection inside a deferred transaction, so every
    /// statement in `f` sees the same snapshot.
    pub(crate) fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let mut conn = self.reader();
        let tx = conn.transaction()?;
        let out = f(&tx)?;
        // Read-only: dropping the transaction just ends the snapshot.
        drop(tx);
        Ok(out)
    }

    /// Pick an idle reader (round-robin), blocking on one only if all are busy.
    fn reader(&self) -> MutexGuard<'_, Connection> {
        let readers = &self.inner.readers;
        let start = self.inner.next_reader.fetch_add(1, Ordering::Relaxed);
        for i in 0..readers.len() {
            let idx = (start + i) % readers.len();
            match readers[idx].try_lock() {
                Ok(guard) => return guard,
                Err(std::sync::TryLockError::Poisoned(p)) => return p.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => continue,
            }
        }
        lock(&readers[start % readers.len()])
    }

    /// Run `PRAGMA optimize` and checkpoint the WAL; cheap, call on idle/shutdown.
    pub fn maintenance(&self) -> Result<()> {
        let conn = self.writer_conn();
        conn.execute_batch("PRAGMA optimize; PRAGMA wal_checkpoint(PASSIVE);")?;
        Ok(())
    }
}

/// Lock a mutex, recovering from poisoning: a panicked holder's open transaction was
/// rolled back when its `Transaction` dropped, so the connection is still usable.
fn lock(m: &Mutex<Connection>) -> MutexGuard<'_, Connection> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
