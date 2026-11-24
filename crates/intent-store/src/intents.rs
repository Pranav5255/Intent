//! Pipeline results: atomic per-day replacement, tree rebuild, stats and deletes.

use std::collections::{BTreeSet, HashMap};

use intent_types::{Intent, PipelineResult, PipelineWarning};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::Store;
use crate::day::{now_ts, validate_date};
use crate::error::{Result, invalid};
use crate::search;

/// A pipeline run served from cache.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CachedRun {
    pub intents: Vec<Intent>,
    pub warnings: Vec<PipelineWarning>,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeleteReport {
    pub deleted_intent_ids: Vec<String>,
    pub deleted_count: usize,
    /// Raw events deleted with the intents (forget only).
    #[serde(default)]
    pub deleted_event_count: usize,
    /// Dates whose intents were removed or must be rebuilt.
    #[serde(default)]
    pub affected_dates: Vec<String>,
}

/// Lightweight root row (no tree), e.g. for resume selection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RootIntent {
    pub intent: Intent,
    pub project: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DayBucket {
    pub date: String,
    pub intent_count: i64,
    pub duration_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LabelCount {
    pub label: String,
    pub count: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectCount {
    pub project: String,
    pub count: i64,
}

/// Aggregates over root intents in an inclusive date range (0.1 `get_intent_stats`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntentStatsReport {
    pub date_from: String,
    pub date_to: String,
    pub project: Option<String>,
    pub intent_count: i64,
    pub total_duration_seconds: i64,
    pub event_count: i64,
    pub by_date: Vec<DayBucket>,
    pub top_labels: Vec<LabelCount>,
    pub projects: Vec<ProjectCount>,
}

/// Accepts `foo` or `project:foo`; returns the bare, trimmed project name.
pub(crate) fn normalize_project(p: &str) -> &str {
    let p = p.trim();
    p.strip_prefix("project:").unwrap_or(p).trim()
}

/// Move a tree into pre-order `(node_without_children)` form, forcing every child's
/// `parent_id` to match its actual parent so the reload is faithful.
fn flatten_owned(mut node: Intent, out: &mut Vec<Intent>) {
    let kids = std::mem::take(&mut node.children);
    let id = node.id.clone();
    out.push(node);
    for mut kid in kids {
        kid.parent_id = Some(id.clone());
        flatten_owned(kid, out);
    }
}

/// Rebuild trees from pre-order rows. `is_root` decides which nodes start a tree;
/// non-root nodes whose parent is absent are dropped (as in 0.1).
fn assemble(nodes: Vec<Intent>, is_root: impl Fn(&Intent) -> bool) -> Vec<Intent> {
    let index: HashMap<String, usize> = nodes.iter().enumerate().map(|(i, n)| (n.id.clone(), i)).collect();
    let mut kids: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    let mut roots = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        if is_root(n) {
            roots.push(i);
        } else if let Some(&p) = n.parent_id.as_ref().and_then(|p| index.get(p))
            && p != i
        {
            kids[p].push(i);
        }
    }
    let mut slots: Vec<Option<Intent>> = nodes.into_iter().map(Some).collect();
    fn build(i: usize, slots: &mut [Option<Intent>], kids: &[Vec<usize>], depth_guard: usize) -> Option<Intent> {
        let mut node = slots[i].take()?;
        if depth_guard < 256 {
            node.children = kids[i].iter().filter_map(|&k| build(k, slots, kids, depth_guard + 1)).collect();
        }
        Some(node)
    }
    roots.into_iter().filter_map(|r| build(r, &mut slots, &kids, 0)).collect()
}

fn parse_nodes(conn: &Connection, sql: &str, params: impl rusqlite::Params) -> Result<Vec<Intent>> {
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt.query_map(params, |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for json in rows {
        out.push(serde_json::from_str::<Intent>(&json?)?);
    }
    Ok(out)
}

/// The privacy-safe "insights" projection indexed by FTS: only derived aggregate
/// metadata (shell command families, semantic topic) — never evidence or event text.
fn insights_projection(node: &Intent) -> String {
    let mut words: BTreeSet<String> = BTreeSet::new();
    for item in &node.insights.shell {
        if let Some(fam) = item.get("command_family").and_then(|v| v.as_str()) {
            words.insert(fam.to_string());
        }
    }
    let mut out: Vec<String> = words.into_iter().collect();
    if let Some(topic) = node.semantic.as_ref().and_then(|s| s.topic.as_deref()) {
        out.push(topic.to_string());
    }
    out.join(" ")
}

/// Insert one flattened node (children already removed). Returns its `pk`.
pub(crate) fn insert_node(
    tx: &Transaction<'_>,
    date: &str,
    node: &Intent,
    ord: i64,
    source_hash: &str,
    now: i64,
) -> Result<i64> {
    debug_assert!(node.children.is_empty());
    let json = serde_json::to_string(node)?;
    let project = node.project_tag().map(|p| p.trim().to_string()).filter(|p| !p.is_empty());
    tx.prepare_cached(
        "INSERT INTO intents(id, date, parent_id, ord, start_ts, end_ts, label, summary, depth, project,
                             confidence, event_count, duration_seconds, tags, insights, intent_json,
                             source_hash, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
    )?
    .execute(params![
        node.id,
        date,
        node.parent_id,
        ord,
        node.start_ts,
        node.end_ts,
        node.label,
        node.summary,
        node.depth,
        project,
        node.confidence,
        node.stats.event_count,
        node.stats.duration_seconds,
        node.tags.join(" "),
        insights_projection(node),
        json,
        source_hash,
        now,
    ])?;
    Ok(tx.last_insert_rowid())
}

/// Delete intents by pk, including their vectors (FTS rows go via trigger).
pub(crate) fn delete_pks(tx: &Transaction<'_>, pks: &[i64]) -> Result<()> {
    let vec = search::vec_enabled(tx)?;
    let mut del = tx.prepare_cached("DELETE FROM intents WHERE pk = ?1")?;
    for pk in pks {
        if vec {
            tx.prepare_cached("DELETE FROM intent_vec WHERE rowid = ?1")?.execute([pk])?;
        }
        del.execute([pk])?;
    }
    Ok(())
}

pub(crate) fn collect_id_pk(
    tx: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<(String, i64, String)>> {
    let mut stmt = tx.prepare_cached(sql)?;
    let rows = stmt.query_map(params, |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Delete the given seed intents plus all their descendants; returns deleted (id, date).
pub(crate) fn delete_subtrees(tx: &Transaction<'_>, seed_sql: &str, param: &str) -> Result<Vec<(String, String)>> {
    let sql = format!(
        "WITH RECURSIVE sub(id) AS ({seed_sql} UNION SELECT i.id FROM intents i JOIN sub ON i.parent_id = sub.id)
         SELECT i.id, i.pk, i.date FROM intents i JOIN sub ON sub.id = i.id ORDER BY i.id"
    );
    let rows = collect_id_pk(tx, &sql, [param])?;
    let pks: Vec<i64> = rows.iter().map(|r| r.1).collect();
    delete_pks(tx, &pks)?;
    Ok(rows.into_iter().map(|(id, _, date)| (id, date)).collect())
}

impl Store {
    /// Atomically replace `date`'s intents and pipeline-run record with `result`.
    ///
    /// Each tree node becomes one row whose `intent_json` is the node *without*
    /// children. Embeddings of nodes whose id, label and summary are unchanged are
    /// carried over so re-runs do not force re-embedding.
    pub fn save_pipeline_run(&self, date: &str, result: &PipelineResult) -> Result<()> {
        validate_date(date)?;
        let mut nodes = Vec::new();
        for mut root in result.intents.iter().cloned() {
            root.parent_id = None;
            flatten_owned(root, &mut nodes);
        }
        let mut seen = std::collections::HashSet::new();
        if let Some(dup) = nodes.iter().find(|n| !seen.insert(n.id.as_str())) {
            return Err(invalid(format!("duplicate intent id {:?} in pipeline result", dup.id)));
        }
        let warnings_json = serde_json::to_string(&result.warnings)?;
        let now = now_ts();

        self.write(|tx| {
            // Old rows to replace: this date's rows plus any row reusing a new id.
            let mut old: HashMap<String, (i64, String)> = HashMap::new();
            for (id, pk, text) in
                collect_id_pk(tx, "SELECT id, pk, label || char(31) || summary FROM intents WHERE date = ?1", [date])?
            {
                old.insert(id, (pk, text));
            }
            for n in &nodes {
                if !old.contains_key(&n.id) {
                    for (id, pk, text) in collect_id_pk(
                        tx,
                        "SELECT id, pk, label || char(31) || summary FROM intents WHERE id = ?1",
                        [&n.id],
                    )? {
                        old.insert(id, (pk, text));
                    }
                }
            }

            // Carry over still-valid embeddings.
            let vec = search::vec_enabled(tx)?;
            let mut carried: HashMap<String, Vec<u8>> = HashMap::new();
            if vec {
                for n in &nodes {
                    if let Some((pk, text)) = old.get(&n.id)
                        && *text == format!("{}\u{1f}{}", n.label, n.summary)
                        && let Some(blob) = tx
                            .prepare_cached("SELECT embedding FROM intent_vec WHERE rowid = ?1")?
                            .query_row([pk], |r| r.get::<_, Vec<u8>>(0))
                            .optional()?
                    {
                        carried.insert(n.id.clone(), blob);
                    }
                }
            }

            let pks: Vec<i64> = old.values().map(|v| v.0).collect();
            delete_pks(tx, &pks)?;
            tx.execute("DELETE FROM pipeline_runs WHERE date = ?1", [date])?;
            tx.execute(
                "INSERT INTO pipeline_runs(date, source_hash, status, warnings_json, started_at, completed_at)
                 VALUES (?1, ?2, 'complete', ?3, ?4, ?4)",
                params![date, result.source_hash, warnings_json, now],
            )?;
            for (ord, node) in nodes.iter().enumerate() {
                let pk = insert_node(tx, date, node, ord as i64, &result.source_hash, now)?;
                if let Some(blob) = carried.get(&node.id) {
                    tx.prepare_cached("INSERT INTO intent_vec(rowid, embedding) VALUES (?1, ?2)")?
                        .execute(params![pk, blob])?;
                }
            }
            Ok(())
        })
    }

    /// The cached run for `(date, source_hash)`, if the pipeline already ran on
    /// exactly this input.
    pub fn cached_run(&self, date: &str, source_hash: &str) -> Result<Option<CachedRun>> {
        validate_date(date)?;
        self.read(|c| {
            let run: Option<(String, Option<i64>)> = c
                .prepare_cached(
                    "SELECT warnings_json, completed_at FROM pipeline_runs WHERE date = ?1 AND source_hash = ?2",
                )?
                .query_row([date, source_hash], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            let Some((warnings_json, completed_at)) = run else { return Ok(None) };
            let nodes = parse_nodes(
                c,
                "SELECT intent_json FROM intents WHERE date = ?1 AND source_hash = ?2 ORDER BY ord",
                [date, source_hash],
            )?;
            Ok(Some(CachedRun {
                intents: assemble(nodes, |n| n.parent_id.is_none()),
                warnings: serde_json::from_str(&warnings_json)?,
                completed_at,
            }))
        })
    }

    /// All intent trees for `date`, in pipeline order.
    pub fn intents_for_date(&self, date: &str) -> Result<Vec<Intent>> {
        validate_date(date)?;
        self.read(|c| {
            let nodes = parse_nodes(c, "SELECT intent_json FROM intents WHERE date = ?1 ORDER BY ord", [date])?;
            Ok(assemble(nodes, |n| n.parent_id.is_none()))
        })
    }

    /// One intent (any depth) with its full subtree of children.
    pub fn intent_by_id(&self, id: &str) -> Result<Option<Intent>> {
        self.read(|c| {
            let nodes = parse_nodes(
                c,
                "WITH RECURSIVE sub(id) AS (
                     SELECT id FROM intents WHERE id = ?1
                     UNION SELECT i.id FROM intents i JOIN sub ON i.parent_id = sub.id)
                 SELECT i.intent_json FROM intents i JOIN sub ON sub.id = i.id ORDER BY i.ord",
                [id],
            )?;
            Ok(assemble(nodes, |n| n.id == id).into_iter().next())
        })
    }

    /// Root intents (no children), newest `end_ts` first. `limit = None` returns all.
    pub fn root_intents(&self, limit: Option<usize>) -> Result<Vec<RootIntent>> {
        let lim = limit.map_or(-1, |l| l as i64);
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT intent_json, project FROM intents WHERE depth = 0 AND parent_id IS NULL
                 ORDER BY end_ts DESC, id LIMIT ?1",
            )?;
            let rows = stmt.query_map([lim], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?;
            let mut out = Vec::new();
            for row in rows {
                let (json, project) = row?;
                out.push(RootIntent { intent: serde_json::from_str(&json)?, project });
            }
            Ok(out)
        })
    }

    /// Aggregate root-intent metadata for `[date_from, date_to]`, optionally for one
    /// project, using indexed columns only (no JSON parsing).
    pub fn stats(&self, date_from: &str, date_to: &str, project: Option<&str>) -> Result<IntentStatsReport> {
        validate_date(date_from)?;
        validate_date(date_to)?;
        if date_from > date_to {
            return Err(invalid("date_from must not be later than date_to"));
        }
        let project = project.map(normalize_project).filter(|p| !p.is_empty()).map(str::to_string);
        self.read(|c| {
            const FILTER: &str = "depth = 0 AND date >= ?1 AND date <= ?2 AND (?3 IS NULL OR project = ?3)";
            let p = params![date_from, date_to, project];
            let (intent_count, total_duration_seconds, event_count): (i64, i64, i64) = c
                .prepare_cached(&format!(
                    "SELECT COUNT(*), COALESCE(SUM(duration_seconds), 0), COALESCE(SUM(event_count), 0)
                     FROM intents WHERE {FILTER}"
                ))?
                .query_row(p, |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            let by_date = {
                let mut stmt = c.prepare_cached(&format!(
                    "SELECT date, COUNT(*), COALESCE(SUM(duration_seconds), 0) FROM intents WHERE {FILTER}
                     GROUP BY date ORDER BY date"
                ))?;
                stmt.query_map(p, |r| {
                    Ok(DayBucket { date: r.get(0)?, intent_count: r.get(1)?, duration_seconds: r.get(2)? })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
            };
            let top_labels = {
                let mut stmt = c.prepare_cached(&format!(
                    "SELECT label, COUNT(*) AS n FROM intents WHERE {FILTER} GROUP BY label ORDER BY n DESC, label"
                ))?;
                stmt.query_map(p, |r| Ok(LabelCount { label: r.get(0)?, count: r.get(1)? }))?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            let projects = {
                let mut stmt = c.prepare_cached(&format!(
                    "SELECT project, COUNT(*) AS n FROM intents WHERE {FILTER} AND project IS NOT NULL
                     GROUP BY project ORDER BY n DESC, project"
                ))?;
                stmt.query_map(p, |r| Ok(ProjectCount { project: r.get(0)?, count: r.get(1)? }))?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            Ok(IntentStatsReport {
                date_from: date_from.to_string(),
                date_to: date_to.to_string(),
                project: project.clone(),
                intent_count,
                total_duration_seconds,
                event_count,
                by_date,
                top_labels,
                projects,
            })
        })
    }

    /// Remove all intents and pipeline runs for one date, keeping its raw events so
    /// the pipeline can rebuild it (recompute). To forget a day use
    /// [`Store::forget_date`].
    pub fn delete_date(&self, date: &str) -> Result<DeleteReport> {
        validate_date(date)?;
        self.write(|tx| {
            let rows = collect_id_pk(tx, "SELECT id, pk, date FROM intents WHERE date = ?1 ORDER BY id", [date])?;
            let pks: Vec<i64> = rows.iter().map(|r| r.1).collect();
            delete_pks(tx, &pks)?;
            tx.execute("DELETE FROM pipeline_runs WHERE date = ?1", [date])?;
            let ids: Vec<String> = rows.into_iter().map(|r| r.0).collect();
            Ok(DeleteReport { deleted_count: ids.len(), deleted_intent_ids: ids, ..Default::default() })
        })
    }
}
