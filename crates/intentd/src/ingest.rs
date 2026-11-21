//! The single ingest path: validation → pause → consent/redaction/blocklist
//! (`intent_capture::admit`) → bounded, batched writer.
//!
//! HTTP companions (`POST /v1/event`) and the in-process collectors both go through
//! [`ingest`]; only the waiting behaviour differs (HTTP waits for the durable write and
//! fails fast with 503 when the queue is full, collectors apply back-pressure).
//!
//! The writer mirrors the 1.x `EventWriter`: one task drains a bounded queue in
//! batches of at most `max_batch` events or `batch_wait` (whichever first) and writes
//! each batch in ONE SQLite transaction on a blocking thread.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use intent_capture::{Admission, EventIn, admit, validate_event};
use intent_store::{InsertOutcome, Store, now_ts};
use intent_types::RawEvent;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use crate::state::Shared;

#[derive(Debug, Clone)]
pub struct WriterConfig {
    pub queue_capacity: usize,
    pub max_batch: usize,
    pub batch_wait: Duration,
    pub submit_timeout: Duration,
    /// Artificial delay before each batch write (tests use it to fill the queue).
    pub write_delay: Duration,
}

impl Default for WriterConfig {
    fn default() -> Self {
        Self {
            queue_capacity: 1024,
            max_batch: 100,
            batch_wait: Duration::from_millis(25),
            submit_timeout: Duration::from_secs(5),
            write_delay: Duration::ZERO,
        }
    }
}

impl WriterConfig {
    /// 1.x tuning variables (`INTENT_INGEST_*`), ignoring invalid values.
    pub fn from_env() -> Self {
        let get = |key: &str| std::env::var(key).ok().and_then(|v| v.trim().parse::<u64>().ok());
        let d = Self::default();
        Self {
            queue_capacity: get("INTENT_INGEST_QUEUE_CAPACITY")
                .filter(|v| *v > 0)
                .map_or(d.queue_capacity, |v| v as usize),
            max_batch: get("INTENT_INGEST_BATCH_SIZE").filter(|v| *v > 0).map_or(d.max_batch, |v| v as usize),
            batch_wait: get("INTENT_INGEST_BATCH_WAIT_MS").map_or(d.batch_wait, Duration::from_millis),
            submit_timeout: get("INTENT_INGEST_SUBMIT_TIMEOUT_SECONDS")
                .filter(|v| *v > 0)
                .map_or(d.submit_timeout, Duration::from_secs),
            write_delay: Duration::ZERO,
        }
    }
}

// ---------------------------------------------------------------------------
// Metrics (no activity content, just counters)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct MetricsInner {
    accepted: u64,
    inserted: u64,
    duplicates: u64,
    batches: u64,
    write_failures: u64,
    submit_timeouts: u64,
    dropped: BTreeMap<String, u64>,
    high_water: usize,
    last_write_at: Option<i64>,
    last_error: Option<String>,
    total_wait_ms: f64,
    total_write_ms: f64,
}

pub struct Metrics {
    inner: Mutex<MetricsInner>,
    capacity: usize,
    started_at: i64,
}

impl Metrics {
    fn new(capacity: usize) -> Self {
        Self { inner: Mutex::new(MetricsInner::default()), capacity, started_at: now_ts() }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MetricsInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn record_drop(&self, reason: &str) {
        *self.lock().dropped.entry(reason.to_string()).or_default() += 1;
    }

    fn record_enqueue(&self, depth: usize) {
        let mut m = self.lock();
        m.high_water = m.high_water.max(depth);
    }

    fn record_batch(&self, outcomes: &[InsertOutcome], wait_ms: f64, write_ms: f64) {
        let mut m = self.lock();
        let inserted = outcomes.iter().filter(|o| o.inserted).count() as u64;
        m.accepted += outcomes.len() as u64;
        m.inserted += inserted;
        m.duplicates += outcomes.len() as u64 - inserted;
        m.batches += 1;
        m.last_write_at = Some(now_ts());
        m.last_error = None;
        m.total_wait_ms += wait_ms;
        m.total_write_ms += write_ms;
    }

    fn record_failure(&self, error: &str) {
        let mut m = self.lock();
        m.write_failures += 1;
        m.last_error = Some(error.to_string());
    }

    fn record_timeout(&self) {
        self.lock().submit_timeouts += 1;
    }

    /// 1.x `IngestionMetrics.snapshot` shape.
    pub fn snapshot(&self, queue_depth: usize, accepting: bool) -> Value {
        let m = self.lock();
        let round2 = |v: f64| (v * 100.0).round() / 100.0;
        json!({
            "state": if m.last_error.is_some() { "degraded" } else if accepting { "ready" } else { "stopping" },
            "started_at": self.started_at,
            "queue_depth": queue_depth,
            "queue_capacity": self.capacity,
            "queue_high_watermark": m.high_water,
            "accepted": m.accepted,
            "inserted": m.inserted,
            "duplicates": m.duplicates,
            "dropped": m.dropped.values().sum::<u64>(),
            "dropped_by_reason": m.dropped,
            "batches": m.batches,
            "write_failures": m.write_failures,
            "submit_timeouts": m.submit_timeouts,
            "last_write_at": m.last_write_at,
            "last_error": m.last_error,
            "average_queue_wait_ms": if m.accepted > 0 { round2(m.total_wait_ms / m.accepted as f64) } else { 0.0 },
            "average_write_ms": if m.batches > 0 { round2(m.total_write_ms / m.batches as f64) } else { 0.0 },
        })
    }
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

type Reply = oneshot::Sender<Result<InsertOutcome, String>>;

/// Longest gap between two events that still joins them into one batch.
const LINGER: Duration = Duration::from_millis(2);

enum Msg {
    Event { event: RawEvent, enqueued: Instant, reply: Option<Reply> },
    Flush(oneshot::Sender<()>),
}

/// Called (on the writer task) with the newly inserted events of each batch.
pub type InsertHook = Arc<dyn Fn(&[&RawEvent]) + Send + Sync>;

#[derive(Debug)]
pub enum SubmitError {
    QueueFull,
    Stopping,
    Timeout,
    Failed(String),
}

#[derive(Clone)]
pub struct Writer {
    tx: mpsc::Sender<Msg>,
    metrics: Arc<Metrics>,
    accepting: Arc<AtomicBool>,
    config: WriterConfig,
}

impl Writer {
    pub fn spawn(store: Store, config: WriterConfig, on_insert: InsertHook) -> Writer {
        let (tx, rx) = mpsc::channel(config.queue_capacity.max(1));
        let metrics = Arc::new(Metrics::new(config.queue_capacity));
        tokio::spawn(run(rx, store, Arc::clone(&metrics), config.clone(), on_insert));
        Writer { tx, metrics, accepting: Arc::new(AtomicBool::new(true)), config }
    }

    pub fn metrics(&self) -> &Metrics {
        &self.metrics
    }

    pub fn queue_depth(&self) -> usize {
        self.tx.max_capacity() - self.tx.capacity()
    }

    pub fn snapshot(&self) -> Value {
        self.metrics.snapshot(self.queue_depth(), self.accepting.load(Ordering::Relaxed))
    }

    /// Persist one event and wait for its batch to commit (HTTP path).
    pub async fn submit(&self, event: RawEvent) -> Result<InsertOutcome, SubmitError> {
        if !self.accepting.load(Ordering::Relaxed) {
            self.metrics.record_drop("writer_stopping");
            return Err(SubmitError::Stopping);
        }
        let (reply, rx) = oneshot::channel();
        match self.tx.try_send(Msg::Event { event, enqueued: Instant::now(), reply: Some(reply) }) {
            Ok(()) => self.metrics.record_enqueue(self.queue_depth()),
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.metrics.record_drop("queue_full");
                return Err(SubmitError::QueueFull);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.metrics.record_drop("writer_stopping");
                return Err(SubmitError::Stopping);
            }
        }
        match tokio::time::timeout(self.config.submit_timeout, rx).await {
            Ok(Ok(Ok(outcome))) => Ok(outcome),
            Ok(Ok(Err(err))) => Err(SubmitError::Failed(err)),
            Ok(Err(_)) => Err(SubmitError::Failed("writer dropped the event".into())),
            Err(_) => {
                self.metrics.record_timeout();
                Err(SubmitError::Timeout)
            }
        }
    }

    /// Queue one event without waiting for the write (collector path). Waits for
    /// queue capacity instead of dropping, which back-pressures the collectors.
    pub async fn enqueue(&self, event: RawEvent) -> Result<(), SubmitError> {
        if !self.accepting.load(Ordering::Relaxed) {
            self.metrics.record_drop("writer_stopping");
            return Err(SubmitError::Stopping);
        }
        self.tx
            .send(Msg::Event { event, enqueued: Instant::now(), reply: None })
            .await
            .map_err(|_| SubmitError::Stopping)?;
        self.metrics.record_enqueue(self.queue_depth());
        Ok(())
    }

    /// Wait until everything queued before this call has been written.
    pub async fn flush(&self) {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(Msg::Flush(tx)).await.is_ok() {
            let _ = rx.await;
        }
    }

    /// Stop accepting new events (shutdown); call [`Writer::flush`] afterwards.
    pub fn close(&self) {
        self.accepting.store(false, Ordering::Relaxed);
    }
}

async fn run(
    mut rx: mpsc::Receiver<Msg>,
    store: Store,
    metrics: Arc<Metrics>,
    config: WriterConfig,
    on_insert: InsertHook,
) {
    while let Some(first) = rx.recv().await {
        let mut batch: Vec<(RawEvent, Instant, Option<Reply>)> = Vec::new();
        let mut flushes: Vec<oneshot::Sender<()>> = Vec::new();
        match first {
            Msg::Event { event, enqueued, reply } => batch.push((event, enqueued, reply)),
            Msg::Flush(done) => {
                let _ = done.send(());
                continue;
            }
        }
        // The batch stays open while events keep arriving (gaps ≤ LINGER), for at most
        // `batch_wait` and `max_batch` events. 1.x always waited the full window, which
        // capped a client that waits for its 201 at ~1 event per 25 ms.
        let deadline = Instant::now() + config.batch_wait;
        while batch.len() < config.max_batch {
            let until = deadline.min(Instant::now() + LINGER);
            match tokio::time::timeout_at(until, rx.recv()).await {
                Ok(Some(Msg::Event { event, enqueued, reply })) => batch.push((event, enqueued, reply)),
                Ok(Some(Msg::Flush(done))) => {
                    flushes.push(done);
                    break;
                }
                Ok(None) | Err(_) => break,
            }
        }
        write_batch(&store, &metrics, &on_insert, batch, config.write_delay).await;
        for done in flushes {
            let _ = done.send(());
        }
    }
}

async fn write_batch(
    store: &Store,
    metrics: &Metrics,
    on_insert: &InsertHook,
    batch: Vec<(RawEvent, Instant, Option<Reply>)>,
    config_delay: Duration,
) {
    if !config_delay.is_zero() {
        tokio::time::sleep(config_delay).await;
    }
    let started = Instant::now();
    let wait_ms: f64 = batch.iter().map(|(_, enq, _)| started.duration_since(*enq).as_secs_f64() * 1000.0).sum();
    let mut events = Vec::with_capacity(batch.len());
    let mut replies = Vec::with_capacity(batch.len());
    for (event, _, reply) in batch {
        events.push(event);
        replies.push(reply);
    }
    let writer_store = store.clone();
    let result = tokio::task::spawn_blocking(move || writer_store.insert_events(&events))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r.map_err(|e| e.to_string()));
    match result {
        Ok(report) => {
            let write_ms = started.elapsed().as_secs_f64() * 1000.0;
            metrics.record_batch(&report.outcomes, wait_ms, write_ms);
            let inserted: Vec<&RawEvent> = report.outcomes.iter().filter(|o| o.inserted).map(|o| &o.event).collect();
            if !inserted.is_empty() {
                on_insert(&inserted);
            }
            for (reply, outcome) in replies.into_iter().zip(report.outcomes) {
                if let Some(reply) = reply {
                    let _ = reply.send(Ok(outcome));
                }
            }
        }
        Err(err) => {
            tracing::error!(error = %err, "event batch write failed");
            metrics.record_failure("StoreError");
            for reply in replies.into_iter().flatten() {
                let _ = reply.send(Err(err.clone()));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Policy pipeline shared by HTTP and collectors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum Ingested {
    Stored(InsertOutcome),
    Queued,
    Dropped(&'static str),
}

#[derive(Debug)]
pub enum IngestError {
    /// 422: the event failed validation or redaction.
    Invalid(String),
    /// 503: the writer cannot accept the event.
    Unavailable(SubmitError),
}

/// How the caller wants to hand the event to the writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// Wait for the durable write (HTTP).
    Wait,
    /// Enqueue with back-pressure (collectors).
    Enqueue,
}

/// Validate a raw JSON body the way 1.x pydantic did.
pub fn parse_event(body: &[u8]) -> Result<RawEvent, IngestError> {
    let event: EventIn = serde_json::from_slice(body).map_err(|e| IngestError::Invalid(e.to_string()))?;
    validate_event(event).map_err(|e| IngestError::Invalid(e.message))
}

/// Re-validate an event produced in-process (collectors use the same rules).
pub fn revalidate(event: RawEvent) -> Result<RawEvent, IngestError> {
    let input = EventIn {
        id: event.id,
        schema_version: event.schema_version,
        ts: event.ts,
        source: event.source,
        kind: event.kind,
        payload: event.payload,
    };
    validate_event(input).map_err(|e| IngestError::Invalid(e.message))
}

/// pause → admit → writer.
pub async fn ingest(shared: &Shared, event: RawEvent, delivery: Delivery) -> Result<Ingested, IngestError> {
    let metrics = shared.writer.metrics();
    if shared.pause.is_paused() {
        metrics.record_drop("capture_paused");
        return Ok(Ingested::Dropped("capture_paused"));
    }
    // Never fails: a broken config file falls back to its safe side (see `admission_policy`).
    let policy = shared.config.admission_policy();
    let event = match admit(event, &policy) {
        Ok(Admission::Accept(event)) => event,
        Ok(Admission::Drop(reason)) => {
            metrics.record_drop(reason.as_str());
            return Ok(Ingested::Dropped(reason.as_str()));
        }
        Err(err) => {
            metrics.record_drop("redaction_rejected");
            tracing::warn!(source = %"event", "event rejected by redaction");
            return Err(IngestError::Invalid(err.message));
        }
    };
    match delivery {
        Delivery::Wait => shared.writer.submit(event).await.map(Ingested::Stored).map_err(IngestError::Unavailable),
        Delivery::Enqueue => {
            shared.writer.enqueue(event).await.map(|()| Ingested::Queued).map_err(IngestError::Unavailable)
        }
    }
}
