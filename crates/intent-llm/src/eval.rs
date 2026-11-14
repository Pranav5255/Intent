//! Correlation quality evaluation over labelled fixtures.
//!
//! Each fixture is a session of events with an expected task label per event. The
//! refiner's clusters are scored with pairwise precision/recall/F1 over same-cluster
//! event pairs and the Adjusted Rand Index.

use std::collections::HashMap;
use std::time::Instant;

use intent_types::{EventEntities, Family, FileKind, NormalizedEvent};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::traits::SemanticRefiner;

pub const BASE_TS: i64 = 1_700_000_000;

/// Fixtures shipped with the crate (also loadable from disk by the `intent-eval` binary).
pub const BUILTIN_FIXTURES: &[(&str, &str)] = &[
    ("auth_bugfix_with_docs", include_str!("../fixtures/eval/auth_bugfix_with_docs.json")),
    ("interleaved_projects", include_str!("../fixtures/eval/interleaved_projects.json")),
    ("research_writing", include_str!("../fixtures/eval/research_writing.json")),
    ("ops_incident", include_str!("../fixtures/eval/ops_incident.json")),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixtureEvent {
    pub id: String,
    /// Seconds from the fixture start.
    pub t: i64,
    pub source: String,
    #[serde(rename = "type")]
    pub kind: String,
    /// Expected task label (ground truth).
    pub task: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub app: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fixture {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub events: Vec<FixtureEvent>,
}

impl Fixture {
    pub fn parse(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| e.to_string())
    }

    pub fn builtin() -> Vec<Fixture> {
        BUILTIN_FIXTURES.iter().map(|(_, text)| Fixture::parse(text).expect("builtin fixture parses")).collect()
    }

    /// Normalized session plus ground-truth labels (parallel to the session).
    pub fn session(&self) -> (Vec<NormalizedEvent>, Vec<String>) {
        let mut events = Vec::new();
        let mut labels = Vec::new();
        for (ordinal, e) in self.events.iter().enumerate() {
            let family = Family::from_source(&e.source, &e.kind);
            let file_kind = e.file.as_deref().map(|f| {
                if f.ends_with(".pdf") {
                    FileKind::Pdf
                } else if f.ends_with(".png") || f.ends_with(".jpg") {
                    FileKind::Image
                } else {
                    FileKind::Code
                }
            });
            let command_family = e.command.as_deref().and_then(|c| c.split_whitespace().next()).map(str::to_owned);
            let mut payload = Map::new();
            for (k, v) in [("app", &e.app), ("title", &e.title), ("cmd", &e.command)] {
                if let Some(v) = v {
                    payload.insert(k.into(), json!(v));
                }
            }
            if let Some(d) = &e.domain {
                payload.insert("url".into(), json!(format!("https://{d}/")));
            }
            let raw = json!({"id": e.id, "ts": BASE_TS + e.t, "source": e.source, "type": e.kind, "payload": Value::Object(payload)});
            events.push(NormalizedEvent {
                id: e.id.clone(),
                ts: BASE_TS + e.t,
                ordinal,
                source: e.source.clone(),
                family,
                category: e.kind.clone(),
                text: [e.title.as_deref(), e.command.as_deref(), e.file.as_deref()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" "),
                entities: EventEntities {
                    project_paths: e.project.iter().cloned().collect(),
                    file_path: e.file.as_ref().map(|f| format!("{}/{f}", e.project.as_deref().unwrap_or("/tmp"))),
                    file_name: e.file.clone(),
                    file_kind,
                    domain: e.domain.clone(),
                    title: e.title.clone(),
                    command: e.command.clone(),
                    command_family,
                    cwd: e.project.clone().filter(|_| e.source == "shell"),
                    exit_code: (e.source == "shell").then_some(0),
                    context_terms: vec![],
                },
                signals: Default::default(),
                evidence: vec![],
                raw: raw.as_object().cloned().unwrap_or_default(),
            });
            labels.push(e.task.clone());
        }
        (events, labels)
    }

    /// A simple deterministic prior: consecutive events of the same family.
    pub fn deterministic(session: &[NormalizedEvent]) -> Vec<Vec<usize>> {
        let mut out: Vec<Vec<usize>> = Vec::new();
        for (i, e) in session.iter().enumerate() {
            match out.last_mut() {
                Some(last) if session[*last.last().unwrap_or(&0)].family == e.family => last.push(i),
                _ => out.push(vec![i]),
            }
        }
        out
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct PairScores {
    pub precision: f64,
    pub recall: f64,
    pub f1: f64,
    pub ari: f64,
}

/// Cluster assignment per event from cluster index lists.
pub fn assignment(n: usize, clusters: &[Vec<usize>]) -> Vec<usize> {
    let mut out: Vec<usize> = (0..n).map(|i| clusters.len() + i).collect();
    for (c, members) in clusters.iter().enumerate() {
        for &i in members {
            if i < n {
                out[i] = c;
            }
        }
    }
    out
}

fn choose2(n: u64) -> f64 {
    (n * n.saturating_sub(1) / 2) as f64
}

/// Pairwise precision/recall/F1 of same-cluster pairs and the Adjusted Rand Index.
pub fn score<A: Eq + std::hash::Hash, B: Eq + std::hash::Hash>(predicted: &[A], expected: &[B]) -> PairScores {
    let n = predicted.len().min(expected.len());
    let (mut tp, mut fp, mut fneg) = (0u64, 0u64, 0u64);
    for i in 0..n {
        for j in (i + 1)..n {
            match (predicted[i] == predicted[j], expected[i] == expected[j]) {
                (true, true) => tp += 1,
                (true, false) => fp += 1,
                (false, true) => fneg += 1,
                _ => {}
            }
        }
    }
    let precision = if tp + fp == 0 { 1.0 } else { tp as f64 / (tp + fp) as f64 };
    let recall = if tp + fneg == 0 { 1.0 } else { tp as f64 / (tp + fneg) as f64 };
    let f1 = if precision + recall == 0.0 { 0.0 } else { 2.0 * precision * recall / (precision + recall) };

    let mut contingency: HashMap<(&A, &B), u64> = HashMap::new();
    let mut rows: HashMap<&A, u64> = HashMap::new();
    let mut cols: HashMap<&B, u64> = HashMap::new();
    for i in 0..n {
        *contingency.entry((&predicted[i], &expected[i])).or_default() += 1;
        *rows.entry(&predicted[i]).or_default() += 1;
        *cols.entry(&expected[i]).or_default() += 1;
    }
    let index: f64 = contingency.values().map(|&v| choose2(v)).sum();
    let sum_rows: f64 = rows.values().map(|&v| choose2(v)).sum();
    let sum_cols: f64 = cols.values().map(|&v| choose2(v)).sum();
    let total = choose2(n as u64);
    let expected_index = if total == 0.0 { 0.0 } else { sum_rows * sum_cols / total };
    let max_index = (sum_rows + sum_cols) / 2.0;
    let ari = if (max_index - expected_index).abs() < f64::EPSILON {
        1.0
    } else {
        (index - expected_index) / (max_index - expected_index)
    };
    PairScores { precision, recall, f1, ari }
}

#[derive(Debug, Clone, Serialize)]
pub struct EvalReport {
    pub fixture: String,
    pub events: usize,
    pub clusters: usize,
    pub scores: PairScores,
    pub baseline: PairScores,
    pub latency_ms: u128,
    pub fallback: Option<String>,
}

/// Run one fixture through a refiner; on fallback the deterministic prior is scored.
pub async fn run_fixture(refiner: &dyn SemanticRefiner, fixture: &Fixture) -> EvalReport {
    let (session, labels) = fixture.session();
    let prior = Fixture::deterministic(&session);
    let baseline = score(&assignment(session.len(), &prior), &labels);
    let started = Instant::now();
    let outcome = refiner.refine(&session, &prior).await;
    let latency_ms = started.elapsed().as_millis();
    let (clusters, fallback) = match outcome {
        Ok(o) => (o.clusters, None),
        Err(f) => (prior.clone(), Some(format!("{f:?}"))),
    };
    EvalReport {
        fixture: fixture.name.clone(),
        events: session.len(),
        clusters: clusters.len(),
        scores: score(&assignment(session.len(), &clusters), &labels),
        baseline,
        latency_ms,
        fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perfect_and_degenerate_scores() {
        let s = score(&[0, 0, 1, 1], &["a", "a", "b", "b"]);
        assert_eq!((s.precision, s.recall, s.f1), (1.0, 1.0, 1.0));
        assert!((s.ari - 1.0).abs() < 1e-9);
        let all_one = score(&[0, 0, 0, 0], &["a", "a", "b", "b"]);
        assert!((all_one.recall - 1.0).abs() < 1e-9);
        assert!((all_one.precision - 2.0 / 6.0).abs() < 1e-9);
        assert!(all_one.ari.abs() < 1e-9);
    }

    #[test]
    fn builtin_fixtures_parse() {
        let fixtures = Fixture::builtin();
        assert!(fixtures.len() >= 3);
        for f in fixtures {
            let (session, labels) = f.session();
            assert_eq!(session.len(), labels.len());
            assert!(session.windows(2).all(|w| w[0].ts <= w[1].ts), "{} is chronological", f.name);
            let distinct: std::collections::HashSet<_> = labels.iter().collect();
            assert!(distinct.len() >= 2);
        }
    }
}
