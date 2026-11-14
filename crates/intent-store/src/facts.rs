//! Small user-facts memory (subject–predicate–object triples), replacing mem0.
//!
//! Facts are deduplicated case-insensitively on `(subject, predicate, object)`;
//! re-observing a fact extends `last_seen_ts` and keeps the highest confidence.
//! Optional embeddings enable brute-force cosine lookup (the table stays small).

use rusqlite::{Row, params};
use serde::{Deserialize, Serialize};

use crate::Store;
use crate::error::{Result, invalid};
use crate::search::{blob_to_vec, embedder_spec};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fact {
    pub id: i64,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub source_intent_id: Option<String>,
    pub first_seen_ts: i64,
    pub last_seen_ts: i64,
    pub confidence: f64,
    /// Not serialized: internal to semantic lookup.
    #[serde(skip)]
    pub embedding: Option<Vec<f32>>,
}

/// A fact observation to upsert.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FactInput {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub source_intent_id: Option<String>,
    pub seen_ts: i64,
    pub confidence: f64,
    #[serde(default, skip_serializing)]
    pub embedding: Option<Vec<f32>>,
}

/// Filter for [`Store::facts`]. Empty = most recently seen facts.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FactQuery {
    /// Exact (case-insensitive) subject.
    pub subject: Option<String>,
    /// Substring matched against subject, predicate and object.
    pub text: Option<String>,
    pub limit: usize,
}

const FACT_COLUMNS: &str =
    "id, subject, predicate, object, source_intent_id, first_seen_ts, last_seen_ts, confidence, embedding";

fn fact_from_row(r: &Row<'_>) -> rusqlite::Result<Fact> {
    let blob: Option<Vec<u8>> = r.get(8)?;
    Ok(Fact {
        id: r.get(0)?,
        subject: r.get(1)?,
        predicate: r.get(2)?,
        object: r.get(3)?,
        source_intent_id: r.get(4)?,
        first_seen_ts: r.get(5)?,
        last_seen_ts: r.get(6)?,
        confidence: r.get(7)?,
        embedding: blob.map(|b| blob_to_vec(&b)),
    })
}

/// Escape `%`, `_` and `\` for a `LIKE ... ESCAPE '\'` pattern.
fn like_pattern(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('%');
    for c in s.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let (mut dot, mut na, mut nb) = (0f32, 0f32, 0f32);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 { 0.0 } else { dot / (na.sqrt() * nb.sqrt()) }
}

impl Store {
    /// Insert or refresh a fact. Returns its id.
    pub fn upsert_fact(&self, fact: &FactInput) -> Result<i64> {
        let (s, p, o) = (fact.subject.trim(), fact.predicate.trim(), fact.object.trim());
        if s.is_empty() || p.is_empty() || o.is_empty() {
            return Err(invalid("fact subject, predicate and object must be non-empty"));
        }
        if !(0.0..=1.0).contains(&fact.confidence) {
            return Err(invalid("fact confidence must be within 0..=1"));
        }
        self.write(|tx| {
            let blob = match &fact.embedding {
                Some(v) => {
                    let spec = embedder_spec(tx)?.ok_or(crate::StoreError::EmbeddingsDisabled)?;
                    if v.len() != spec.dims || v.iter().any(|x| !x.is_finite()) {
                        return Err(invalid(format!("fact embedding must have {} finite dims", spec.dims)));
                    }
                    Some(v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>())
                }
                None => None,
            };
            let id = tx
                .prepare_cached(
                    "INSERT INTO facts(subject, predicate, object, source_intent_id, first_seen_ts, last_seen_ts,
                                       confidence, embedding)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7)
                     ON CONFLICT(subject, predicate, object) DO UPDATE SET
                         source_intent_id = COALESCE(excluded.source_intent_id, facts.source_intent_id),
                         first_seen_ts = MIN(facts.first_seen_ts, excluded.first_seen_ts),
                         last_seen_ts = MAX(facts.last_seen_ts, excluded.last_seen_ts),
                         confidence = MAX(facts.confidence, excluded.confidence),
                         embedding = COALESCE(excluded.embedding, facts.embedding)
                     RETURNING id",
                )?
                .query_row(params![s, p, o, fact.source_intent_id, fact.seen_ts, fact.confidence, blob], |r| {
                    r.get(0)
                })?;
            Ok(id)
        })
    }

    /// Facts matching `q`, most recently seen first. `limit` 0 means 50.
    pub fn facts(&self, q: &FactQuery) -> Result<Vec<Fact>> {
        let limit = if q.limit == 0 { 50 } else { q.limit.min(1000) } as i64;
        let text = q.text.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(like_pattern);
        let subject = q.subject.as_deref().map(str::trim);
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {FACT_COLUMNS} FROM facts
                 WHERE (?1 IS NULL OR subject = ?1)
                   AND (?2 IS NULL OR subject LIKE ?2 ESCAPE '\\' OR predicate LIKE ?2 ESCAPE '\\'
                        OR object LIKE ?2 ESCAPE '\\')
                 ORDER BY last_seen_ts DESC, id DESC LIMIT ?3"
            ))?;
            let rows = stmt.query_map(params![subject, text, limit], fact_from_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Facts ranked by cosine similarity to `query_vec` (only facts with embeddings).
    pub fn similar_facts(&self, query_vec: &[f32], limit: usize) -> Result<Vec<(Fact, f32)>> {
        let all = self.read(|c| {
            let mut stmt =
                c.prepare_cached(&format!("SELECT {FACT_COLUMNS} FROM facts WHERE embedding IS NOT NULL"))?;
            let rows = stmt.query_map([], fact_from_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })?;
        let mut scored: Vec<(Fact, f32)> = all
            .into_iter()
            .filter_map(|f| {
                let s = cosine(f.embedding.as_deref()?, query_vec);
                Some((f, s))
            })
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.id.cmp(&b.0.id)));
        scored.truncate(limit);
        Ok(scored)
    }

    /// Delete one fact; `false` if it did not exist.
    pub fn delete_fact(&self, id: i64) -> Result<bool> {
        self.write(|tx| Ok(tx.execute("DELETE FROM facts WHERE id = ?1", [id])? > 0))
    }

    /// Delete every fact. Returns how many were removed.
    pub fn clear_facts(&self) -> Result<usize> {
        self.write(|tx| Ok(tx.execute("DELETE FROM facts", [])?))
    }
}
