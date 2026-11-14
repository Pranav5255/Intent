//! Keyword (FTS5), vector (sqlite-vec) and hybrid search over root intents.
//!
//! * User text is never handed to `MATCH` raw: [`fts_query`] tokenizes it the way
//!   FTS5's `unicode61` tokenizer would, double-quotes every token and adds a prefix
//!   `*` to the last one (search-as-you-type).
//! * Vectors live in a `vec0` table (`intent_vec`, cosine distance) keyed by
//!   `intents.pk`. Its dimension and the embedder identity are recorded in
//!   `embeddings_meta`; switching embedder drops and recreates the table.
//! * [`Store::hybrid_search`] fuses the FTS and KNN rankings with reciprocal-rank
//!   fusion (RRF, k = 60).

use std::collections::HashMap;

use intent_types::SearchHit;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::Store;
use crate::day::{now_ts, validate_date};
use crate::error::{Result, StoreError, invalid};

/// Upper bound on search results per call.
pub const MAX_SEARCH_LIMIT: usize = 100;
/// Max tokens taken from user text (keeps MATCH expressions small).
const MAX_QUERY_TOKENS: usize = 16;
/// RRF constant from the original paper.
const RRF_K: f64 = 60.0;
/// bm25 column weights: label, summary, tags, insights.
const BM25: &str = "bm25(intent_fts, 10.0, 5.0, 2.0, 1.0)";

/// Which embedder produced the stored vectors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbedderSpec {
    /// Stable identity, e.g. `ollama:nomic-embed-text` — changes force a rebuild.
    pub identity: String,
    pub dims: usize,
}

/// A root intent that still needs an embedding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingTodo {
    pub id: String,
    pub label: String,
    pub summary: String,
}

/// Turn arbitrary user text into a safe FTS5 query.
///
/// Tokens are split on anything that is not alphanumeric (mirroring `unicode61`),
/// each token is wrapped in double quotes (so `OR`, `NEAR`, `*`, `(`, `"` and column
/// filters lose their meaning) and the last token gets a prefix `*` unless the text
/// ends in a separator. Tokens are AND-ed, or OR-ed when `any` is true.
/// Returns `None` when the text has no searchable tokens.
pub fn fts_query(text: &str, any: bool) -> Option<String> {
    let tokens: Vec<&str> = text.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).collect();
    if tokens.is_empty() {
        return None;
    }
    let ends_open = text.chars().last().is_some_and(char::is_alphanumeric);
    let tokens = &tokens[..tokens.len().min(MAX_QUERY_TOKENS)];
    let last = tokens.len() - 1;
    let parts: Vec<String> = tokens
        .iter()
        .enumerate()
        .map(|(i, t)| {
            // Tokens are alphanumeric only, so they can never contain a quote.
            if i == last && ends_open { format!("\"{t}\"*") } else { format!("\"{t}\"") }
        })
        .collect();
    Some(parts.join(if any { " OR " } else { " " }))
}

fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub(crate) fn blob_to_vec(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect()
}

fn check_vector(v: &[f32], dims: usize) -> Result<()> {
    if v.len() != dims {
        return Err(invalid(format!("embedding has {} dims, store expects {dims}", v.len())));
    }
    if v.iter().any(|x| !x.is_finite()) {
        return Err(invalid("embedding contains non-finite values"));
    }
    Ok(())
}

/// Current embedder spec, if configured.
pub(crate) fn embedder_spec(conn: &Connection) -> Result<Option<EmbedderSpec>> {
    Ok(conn
        .prepare_cached("SELECT identity, dims FROM embeddings_meta WHERE id = 1")?
        .query_row([], |r| Ok(EmbedderSpec { identity: r.get(0)?, dims: r.get::<_, i64>(1)? as usize }))
        .optional()?)
}

/// Whether `intent_vec` exists (embeddings configured).
pub(crate) fn vec_enabled(conn: &Connection) -> Result<bool> {
    Ok(embedder_spec(conn)?.is_some())
}

/// (Re)create `intent_vec` to match `spec`. Returns `true` if the table was rebuilt.
fn apply_spec(tx: &Transaction<'_>, spec: &EmbedderSpec) -> Result<bool> {
    if spec.dims == 0 || spec.dims > 8192 {
        return Err(invalid("embedding dims must be between 1 and 8192"));
    }
    if embedder_spec(tx)?.as_ref() == Some(spec) {
        return Ok(false);
    }
    tx.execute_batch("DROP TABLE IF EXISTS intent_vec")?;
    tx.execute_batch(&format!(
        "CREATE VIRTUAL TABLE intent_vec USING vec0(embedding float[{}] distance_metric=cosine)",
        spec.dims
    ))?;
    tx.execute(
        "INSERT INTO embeddings_meta(id, identity, dims, updated_at) VALUES (1, ?1, ?2, ?3)
         ON CONFLICT(id) DO UPDATE SET identity = excluded.identity, dims = excluded.dims,
                                       updated_at = excluded.updated_at",
        params![spec.identity, spec.dims as i64, now_ts()],
    )?;
    // Fact vectors from another embedder are meaningless now too.
    tx.execute("UPDATE facts SET embedding = NULL WHERE embedding IS NOT NULL", [])?;
    Ok(true)
}

pub(crate) fn init_embeddings(tx: &Transaction<'_>, spec: Option<&EmbedderSpec>) -> Result<()> {
    if let Some(spec) = spec {
        apply_spec(tx, spec)?;
    }
    Ok(())
}

/// Row data for one search candidate.
struct Candidate {
    id: String,
    label: String,
    summary: String,
    date: String,
    snippet: String,
}

fn fallback_snippet(summary: &str) -> String {
    let mut s: String = summary.chars().take(80).collect();
    if summary.chars().count() > 80 {
        s.push_str("...");
    }
    s
}

/// FTS candidates, best first, with `-bm25` as score. AND first, OR as fallback.
fn fts_candidates(
    c: &Connection,
    text: &str,
    limit: usize,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<Vec<(Candidate, f64)>> {
    let sql = format!(
        "SELECT i.id, i.label, i.summary, i.date,
                snippet(intent_fts, 1, '**', '**', '...', 16),
                snippet(intent_fts, 0, '**', '**', '...', 16),
                {BM25}
         FROM intent_fts JOIN intents i ON i.pk = intent_fts.rowid
         WHERE intent_fts MATCH ?1 AND i.depth = 0
           AND (?2 IS NULL OR i.date >= ?2) AND (?3 IS NULL OR i.date <= ?3)
         ORDER BY {BM25}, i.id LIMIT ?4"
    );
    for any in [false, true] {
        let Some(q) = fts_query(text, any) else { return Ok(Vec::new()) };
        let mut stmt = c.prepare_cached(&sql)?;
        let rows = stmt.query_map(params![q, from, to, limit as i64], |r| {
            let summary: String = r.get(2)?;
            let sum_snip: String = r.get(4)?;
            let label_snip: String = r.get(5)?;
            let snippet = if sum_snip.contains("**") {
                sum_snip
            } else if label_snip.contains("**") {
                label_snip
            } else {
                fallback_snippet(&summary)
            };
            Ok((Candidate { id: r.get(0)?, label: r.get(1)?, summary, date: r.get(3)?, snippet }, -r.get::<_, f64>(6)?))
        })?;
        let out = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        if !out.is_empty() || any || !q.contains(' ') {
            return Ok(out);
        }
    }
    Ok(Vec::new())
}

/// KNN candidates (root intents in range), nearest first, score = 1 - cosine distance.
fn vec_candidates(
    c: &Connection,
    v: &[f32],
    limit: usize,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<Vec<(Candidate, f64)>> {
    let Some(spec) = embedder_spec(c)? else { return Ok(Vec::new()) };
    check_vector(v, spec.dims)?;
    // KNN runs before the depth/date filter, so oversample.
    let k = (limit * 4).clamp(16, 4096) as i64;
    let mut stmt = c.prepare_cached(
        "WITH knn AS (SELECT rowid, distance FROM intent_vec WHERE embedding MATCH ?1 AND k = ?2)
         SELECT i.id, i.label, i.summary, i.date, knn.distance
         FROM knn JOIN intents i ON i.pk = knn.rowid
         WHERE i.depth = 0 AND (?3 IS NULL OR i.date >= ?3) AND (?4 IS NULL OR i.date <= ?4)
         ORDER BY knn.distance, i.id LIMIT ?5",
    )?;
    let rows = stmt.query_map(params![vec_to_blob(v), k, from, to, limit as i64], |r| {
        let summary: String = r.get(2)?;
        Ok((
            Candidate {
                id: r.get(0)?,
                label: r.get(1)?,
                snippet: fallback_snippet(&summary),
                summary,
                date: r.get(3)?,
            },
            1.0 - r.get::<_, f64>(4)?,
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn check_range(from: Option<&str>, to: Option<&str>) -> Result<()> {
    if let Some(d) = from {
        validate_date(d)?;
    }
    if let Some(d) = to {
        validate_date(d)?;
    }
    if let (Some(f), Some(t)) = (from, to)
        && f > t
    {
        return Err(invalid("date_from must not be later than date_to"));
    }
    Ok(())
}

fn hit(c: Candidate, score: f64) -> SearchHit {
    SearchHit { id: c.id, label: c.label, summary: c.summary, date: c.date, highlight_snippet: c.snippet, score }
}

impl Store {
    /// Keyword search over root intents, ranked by weighted bm25, with a `**`-marked
    /// snippet. `limit` is clamped to `1..=100`.
    pub fn search(
        &self,
        query: &str,
        limit: usize,
        date_from: Option<&str>,
        date_to: Option<&str>,
    ) -> Result<Vec<SearchHit>> {
        check_range(date_from, date_to)?;
        let limit = limit.clamp(1, MAX_SEARCH_LIMIT);
        self.read(|c| {
            Ok(fts_candidates(c, query, limit, date_from, date_to)?.into_iter().map(|(c, s)| hit(c, s)).collect())
        })
    }

    /// Keyword + semantic search fused with reciprocal-rank fusion. With
    /// `query_vec = None` (or no embedder configured) this degrades to keyword
    /// ranking with RRF scores. Hits found by both lists rank highest.
    pub fn hybrid_search(
        &self,
        query: &str,
        query_vec: Option<&[f32]>,
        limit: usize,
        date_from: Option<&str>,
        date_to: Option<&str>,
    ) -> Result<Vec<SearchHit>> {
        check_range(date_from, date_to)?;
        let limit = limit.clamp(1, MAX_SEARCH_LIMIT);
        let pool = (limit * 3).max(30);
        self.read(|c| {
            let keyword = fts_candidates(c, query, pool, date_from, date_to)?;
            let semantic = match query_vec {
                Some(v) => vec_candidates(c, v, pool, date_from, date_to)?,
                None => Vec::new(),
            };
            let mut fused: HashMap<String, (Candidate, f64)> = HashMap::new();
            for list in [keyword, semantic] {
                for (rank, (cand, _)) in list.into_iter().enumerate() {
                    let add = 1.0 / (RRF_K + rank as f64 + 1.0);
                    match fused.get_mut(&cand.id) {
                        // Keep the keyword snippet (it has highlights); just add score.
                        Some(entry) => entry.1 += add,
                        None => {
                            fused.insert(cand.id.clone(), (cand, add));
                        }
                    }
                }
            }
            let mut hits: Vec<SearchHit> = fused.into_values().map(|(c, s)| hit(c, s)).collect();
            hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
            hits.truncate(limit);
            Ok(hits)
        })
    }

    /// Pure vector KNN over root intents (score = cosine similarity).
    pub fn vector_search(
        &self,
        query_vec: &[f32],
        limit: usize,
        date_from: Option<&str>,
        date_to: Option<&str>,
    ) -> Result<Vec<SearchHit>> {
        check_range(date_from, date_to)?;
        let limit = limit.clamp(1, MAX_SEARCH_LIMIT);
        self.read(|c| {
            if !vec_enabled(c)? {
                return Err(StoreError::EmbeddingsDisabled);
            }
            Ok(vec_candidates(c, query_vec, limit, date_from, date_to)?.into_iter().map(|(c, s)| hit(c, s)).collect())
        })
    }

    /// Configure (or switch) the embedder. Returns `true` when `intent_vec` was
    /// rebuilt, i.e. every stored intent/fact vector was dropped and must be recomputed.
    pub fn configure_embeddings(&self, spec: &EmbedderSpec) -> Result<bool> {
        self.write(|tx| apply_spec(tx, spec))
    }

    /// The embedder the stored vectors belong to.
    pub fn embedder(&self) -> Result<Option<EmbedderSpec>> {
        self.read(embedder_spec)
    }

    /// Store/replace the vector for `intent_id`. Returns `false` if no such intent.
    pub fn upsert_embedding(&self, intent_id: &str, vector: &[f32]) -> Result<bool> {
        self.write(|tx| {
            let spec = embedder_spec(tx)?.ok_or(StoreError::EmbeddingsDisabled)?;
            check_vector(vector, spec.dims)?;
            let pk: Option<i64> = tx
                .prepare_cached("SELECT pk FROM intents WHERE id = ?1")?
                .query_row([intent_id], |r| r.get(0))
                .optional()?;
            let Some(pk) = pk else { return Ok(false) };
            // vec0 has no UPSERT; delete then insert.
            tx.prepare_cached("DELETE FROM intent_vec WHERE rowid = ?1")?.execute([pk])?;
            tx.prepare_cached("INSERT INTO intent_vec(rowid, embedding) VALUES (?1, ?2)")?
                .execute(params![pk, vec_to_blob(vector)])?;
            Ok(true)
        })
    }

    /// Stored vector for `intent_id`, if any.
    pub fn embedding(&self, intent_id: &str) -> Result<Option<Vec<f32>>> {
        self.read(|c| {
            if !vec_enabled(c)? {
                return Ok(None);
            }
            let blob: Option<Vec<u8>> = c
                .prepare_cached(
                    "SELECT v.embedding FROM intents i JOIN intent_vec v ON v.rowid = i.pk WHERE i.id = ?1",
                )?
                .query_row([intent_id], |r| r.get(0))
                .optional()?;
            Ok(blob.map(|b| blob_to_vec(&b)))
        })
    }

    /// Root intents without a vector (newest first) — the embedding backfill queue.
    pub fn intents_missing_embeddings(&self, limit: usize) -> Result<Vec<EmbeddingTodo>> {
        self.read(|c| {
            if !vec_enabled(c)? {
                return Ok(Vec::new());
            }
            let mut stmt = c.prepare_cached(
                "SELECT i.id, i.label, i.summary FROM intents i
                 WHERE i.depth = 0 AND NOT EXISTS (SELECT 1 FROM intent_vec v WHERE v.rowid = i.pk)
                 ORDER BY i.end_ts DESC, i.id LIMIT ?1",
            )?;
            let rows = stmt.query_map([limit as i64], |r| {
                Ok(EmbeddingTodo { id: r.get(0)?, label: r.get(1)?, summary: r.get(2)? })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_nasty_input() {
        assert_eq!(fts_query(r#"foo" OR bar*("#, false).as_deref(), Some(r#""foo" "OR" "bar""#));
        assert_eq!(fts_query("rust tauri", false).as_deref(), Some(r#""rust" "tauri"*"#));
        assert_eq!(fts_query("rust tauri", true).as_deref(), Some(r#""rust" OR "tauri"*"#));
        assert_eq!(fts_query("label:x NEAR(y)", false).as_deref(), Some(r#""label" "x" "NEAR" "y""#));
        assert_eq!(fts_query(r#"  "" * ( ) "#, false), None);
        assert_eq!(fts_query("café", false).as_deref(), Some(r#""café"*"#));
    }

    #[test]
    fn blob_round_trip() {
        let v = vec![0.5f32, -1.25, 3.0];
        assert_eq!(blob_to_vec(&vec_to_blob(&v)), v);
    }
}
