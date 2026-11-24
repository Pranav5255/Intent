//! Deterministic, topic-aware grouping of one session (0.1 `cluster.py`).
//!
//! Clusters are lists of indices into the session slice, matching
//! [`intent_llm::SemanticOutcome::clusters`].
//!
//! Unlike 0.1, a run's topic is maintained with incremental counters while events
//! are appended (0.1 recomputed it from scratch per event: O(n²)), and invariant
//! violations become warnings instead of `assert`s.

use intent_types::{Family, NormalizedEvent, PipelineWarning, WarningLevel};

use crate::util::OrderedCounter;

/// Events further apart than this start a new cluster.
pub const BOUNDARY_GAP_SECONDS: i64 = 5 * 60;
/// Runs split only by a gap are re-joined when at least this similar.
pub const GAP_MERGE_SIMILARITY: f64 = 0.7;
/// Maximum clusters per session.
pub const MAX_CLUSTERS_PER_SESSION: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryReason {
    Gap,
    CommandPhase,
    TopicShift,
}

impl BoundaryReason {
    pub fn as_str(self) -> &'static str {
        match self {
            BoundaryReason::Gap => "gap",
            BoundaryReason::CommandPhase => "command_phase",
            BoundaryReason::TopicShift => "topic_shift",
        }
    }
}

/// Strongest project / command / file / domain / context-term signals of a group of
/// events (0.1 `_get_topic_score`).
#[derive(Debug, Clone, Default)]
pub struct Topic {
    pub projects: OrderedCounter,
    pub commands: OrderedCounter,
    pub files: OrderedCounter,
    pub domains: OrderedCounter,
    pub terms: OrderedCounter,
}

impl Topic {
    pub fn of<'a, I: IntoIterator<Item = &'a NormalizedEvent>>(events: I) -> Self {
        let mut topic = Topic::default();
        for event in events {
            topic.add(event);
        }
        topic
    }

    pub fn add(&mut self, event: &NormalizedEvent) {
        let e = &event.entities;
        for path in &e.project_paths {
            self.projects.add(path);
        }
        if let Some(cf) = e.command_family.as_deref().filter(|s| !s.is_empty()) {
            self.commands.add(cf);
        }
        if let Some(file) = e.file_name.as_deref().filter(|s| !s.is_empty()) {
            self.files.add(file);
        }
        if let Some(domain) = e.domain.as_deref().filter(|s| !s.is_empty()) {
            self.domains.add(domain);
        }
        for term in &e.context_terms {
            self.terms.add(term);
        }
    }

    pub fn merge(&mut self, other: &Topic) {
        self.projects.merge(&other.projects);
        self.commands.merge(&other.commands);
        self.files.merge(&other.files);
        self.domains.merge(&other.domains);
        self.terms.merge(&other.terms);
    }

    pub fn project(&self) -> Option<&str> {
        self.projects.top()
    }
    pub fn command_family(&self) -> Option<&str> {
        self.commands.top()
    }
    pub fn top_file(&self) -> Option<&str> {
        self.files.top()
    }
    pub fn top_domain(&self) -> Option<&str> {
        self.domains.top()
    }
}

fn values_match(a: Option<&str>, b: Option<&str>) -> Option<bool> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a == b),
        _ => None,
    }
}

fn populated_different(a: Option<&str>, b: Option<&str>) -> bool {
    values_match(a, b) == Some(false)
}

/// Weighted similarity over the signals both topics actually have
/// (project 0.4, command 0.3, file 0.2, domains 0.1, context terms 0.15).
pub fn topic_similarity(a: &Topic, b: &Topic) -> f64 {
    let set_match = |x: &OrderedCounter, y: &OrderedCounter| (!x.is_empty() && !y.is_empty()).then(|| x.intersects(y));
    let comparisons = [
        (0.4, values_match(a.project(), b.project())),
        (0.3, values_match(a.command_family(), b.command_family())),
        (0.2, values_match(a.top_file(), b.top_file())),
        (0.1, set_match(&a.domains, &b.domains)),
        (0.15, set_match(&a.terms, &b.terms)),
    ];
    let mut total = 0.0;
    let mut matched = 0.0;
    let mut any = false;
    for (weight, result) in comparisons {
        if let Some(m) = result {
            any = true;
            total += weight;
            if m {
                matched += weight;
            }
        }
    }
    if any { matched / total } else { 0.0 }
}

/// The single-event view used for boundary checks (no allocation).
struct EventTopic<'a> {
    project: Option<&'a str>,
    command: Option<&'a str>,
    file: Option<&'a str>,
}

impl<'a> EventTopic<'a> {
    fn of(event: &'a NormalizedEvent) -> Self {
        let e = &event.entities;
        EventTopic {
            project: e.project_paths.first().map(String::as_str),
            command: e.command_family.as_deref().filter(|s| !s.is_empty()),
            file: e.file_name.as_deref().filter(|s| !s.is_empty()),
        }
    }
}

fn topic_shift_strong(current: &Topic, event: &EventTopic<'_>) -> bool {
    let project_differs = populated_different(current.project(), event.project);
    let command_differs = populated_different(current.command_family(), event.command);
    let file_differs = populated_different(current.top_file(), event.file);
    (project_differs && (command_differs || file_differs)) || command_differs
}

#[derive(Debug, Clone)]
struct Run {
    indices: Vec<usize>,
    boundary_before: Option<BoundaryReason>,
    topic: Topic,
    meaningful: usize,
    last_command_family: Option<String>,
}

impl Run {
    fn start(index: usize, event: &NormalizedEvent, boundary_before: Option<BoundaryReason>) -> Self {
        let mut run = Run {
            indices: Vec::new(),
            boundary_before,
            topic: Topic::default(),
            meaningful: 0,
            last_command_family: None,
        };
        run.push(index, event);
        run
    }

    fn push(&mut self, index: usize, event: &NormalizedEvent) {
        self.indices.push(index);
        self.topic.add(event);
        if !matches!(event.family, Family::Focus | Family::Idle) {
            self.meaningful += 1;
        }
        if let Some(cf) = event.entities.command_family.as_deref().filter(|s| !s.is_empty()) {
            self.last_command_family = Some(cf.to_owned());
        }
    }

    fn starts_command_phase(&self, event: &NormalizedEvent) -> bool {
        let Some(cf) = event.entities.command_family.as_deref().filter(|s| !s.is_empty()) else {
            return false;
        };
        if event.family != Family::Command || self.meaningful < 2 {
            return false;
        }
        self.last_command_family.as_deref() != Some(cf)
    }
}

/// Output of [`cluster_session`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionClusters {
    pub clusters: Vec<Vec<usize>>,
    /// Parallel to `clusters`: why each cluster started (`None` for the first).
    pub boundaries: Vec<Option<BoundaryReason>>,
    pub warnings: Vec<PipelineWarning>,
}

/// Up to [`MAX_CLUSTERS_PER_SESSION`] chronological, non-overlapping topic clusters.
pub fn cluster_session(session: &[NormalizedEvent]) -> SessionClusters {
    let Some(first) = session.first() else {
        return SessionClusters::default();
    };
    let mut runs: Vec<Run> = Vec::new();
    let mut current = Run::start(0, first, None);
    for (index, event) in session.iter().enumerate().skip(1) {
        let last_ts = session[*current.indices.last().unwrap_or(&0)].ts;
        let boundary = if event.ts - last_ts > BOUNDARY_GAP_SECONDS {
            Some(BoundaryReason::Gap)
        } else if current.starts_command_phase(event) {
            Some(BoundaryReason::CommandPhase)
        } else if topic_shift_strong(&current.topic, &EventTopic::of(event)) {
            Some(BoundaryReason::TopicShift)
        } else {
            None
        };
        match boundary {
            None => current.push(index, event),
            Some(reason) => runs.push(std::mem::replace(&mut current, Run::start(index, event, Some(reason)))),
        }
    }
    runs.push(current);

    let runs = merge_gap_runs(runs);
    let runs = cap_runs(session, runs, MAX_CLUSTERS_PER_SESSION, |_| {});
    let clusters: Vec<Vec<usize>> = runs.iter().map(|r| r.indices.clone()).collect();
    let problems = check_partition(session, &clusters, true);
    if !problems.is_empty() {
        // Cannot happen with the rules above; degrade to one cluster rather than panic.
        return SessionClusters {
            clusters: vec![(0..session.len()).collect()],
            boundaries: vec![None],
            warnings: problems.into_iter().map(|m| error(format!("Cluster invariant violated: {m}"))).collect(),
        };
    }
    SessionClusters { boundaries: runs.iter().map(|r| r.boundary_before).collect(), clusters, warnings: Vec::new() }
}

fn merge_gap_runs(runs: Vec<Run>) -> Vec<Run> {
    let mut merged: Vec<Run> = Vec::with_capacity(runs.len());
    for run in runs {
        if let Some(previous) = merged.last_mut()
            && run.boundary_before == Some(BoundaryReason::Gap)
            && topic_similarity(&previous.topic, &run.topic) >= GAP_MERGE_SIMILARITY
        {
            previous.indices.extend(run.indices);
            previous.topic.merge(&run.topic);
            continue;
        }
        merged.push(run);
    }
    merged
}

/// Repeatedly fold the smallest run into its most similar neighbour until at most
/// `max` remain. `on_merge(left)` reports each fold of `left + 1` into `left`.
fn cap_runs(session: &[NormalizedEvent], mut runs: Vec<Run>, max: usize, mut on_merge: impl FnMut(usize)) -> Vec<Run> {
    let max = max.max(1);
    while runs.len() > max {
        let smallest = (0..runs.len()).min_by_key(|&i| (runs[i].indices.len(), i)).unwrap_or(0);
        let mut target: Option<(f64, usize)> = None;
        for neighbor in [smallest.checked_sub(1), Some(smallest + 1)].into_iter().flatten() {
            if neighbor >= runs.len() {
                continue;
            }
            let sim = topic_similarity(&runs[smallest].topic, &runs[neighbor].topic);
            // Highest similarity wins; ties go to the lower index (0.1 `(sim, -index)` key).
            if target.is_none_or(|(best, _)| sim > best) {
                target = Some((sim, neighbor));
            }
        }
        let Some((_, target)) = target else { break };
        let (left, right) = if smallest < target { (smallest, target) } else { (target, smallest) };
        let absorbed = runs.remove(right);
        let keep = &mut runs[left];
        let in_order = keep.indices.last() < absorbed.indices.first();
        keep.indices.extend(absorbed.indices);
        if in_order {
            // Counter merge == recount over the concatenated, still-sorted events.
            keep.topic.merge(&absorbed.topic);
        } else {
            keep.indices.sort_unstable();
            keep.topic = Topic::of(keep.indices.iter().map(|&i| &session[i]));
        }
        on_merge(left);
    }
    runs
}

/// Apply the cluster cap to arbitrary (e.g. semantically refined) clusters, keeping a
/// parallel payload vector in step via `merge_payload(left, right)`.
pub fn cap_clusters<T>(
    session: &[NormalizedEvent],
    clusters: Vec<Vec<usize>>,
    payload: Vec<T>,
    max: usize,
    mut merge_payload: impl FnMut(T, T, usize, usize) -> T,
) -> (Vec<Vec<usize>>, Vec<T>) {
    let runs: Vec<Run> = clusters
        .into_iter()
        .map(|indices| Run {
            topic: Topic::of(indices.iter().map(|&i| &session[i])),
            indices,
            boundary_before: None,
            meaningful: 0,
            last_command_family: None,
        })
        .collect();
    let sizes: Vec<usize> = runs.iter().map(|r| r.indices.len()).collect();
    let mut slots: Vec<Option<(T, usize)>> = payload.into_iter().zip(sizes).map(Some).collect();
    let runs = cap_runs(session, runs, max, |left| {
        let right = slots.remove(left + 1);
        if let (Some((a, na)), Some((b, nb))) = (slots[left].take(), right) {
            slots[left] = Some((merge_payload(a, b, na, nb), na + nb));
        }
    });
    let payload = slots.into_iter().flatten().map(|(p, _)| p).collect();
    (runs.into_iter().map(|r| r.indices).collect(), payload)
}

/// Check that `clusters` partition `session` (every index exactly once, non-empty,
/// chronological inside each cluster, clusters ordered by first event). With
/// `contiguous`, the flattened order must also be exactly `0..len`.
pub fn check_partition(session: &[NormalizedEvent], clusters: &[Vec<usize>], contiguous: bool) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = vec![false; session.len()];
    for cluster in clusters {
        if cluster.is_empty() {
            problems.push("clusters must not be empty".to_owned());
            continue;
        }
        for &i in cluster {
            match seen.get_mut(i) {
                None => problems.push(format!("event index {i} out of range")),
                Some(true) => problems.push(format!("event index {i} appears more than once")),
                Some(slot) => *slot = true,
            }
        }
        if cluster.windows(2).any(|w| w[0] >= w[1] || session.get(w[0]).map(|e| e.ts) > session.get(w[1]).map(|e| e.ts))
        {
            problems.push("events in a cluster must be chronological".to_owned());
        }
    }
    if seen.iter().any(|s| !s) {
        problems.push("every event must belong to a cluster".to_owned());
    }
    if clusters.windows(2).any(|w| w[0].first() > w[1].first()) {
        problems.push("clusters must be ordered by first event".to_owned());
    }
    if contiguous && problems.is_empty() {
        let flat: Vec<usize> = clusters.iter().flatten().copied().collect();
        if flat != (0..session.len()).collect::<Vec<_>>() {
            problems.push("events must appear exactly once, in order".to_owned());
        }
        if clusters.windows(2).any(|w| session[*w[0].last().unwrap_or(&0)].ts > session[w[1][0]].ts) {
            problems.push("clusters must be chronological".to_owned());
        }
    }
    problems
}

fn error(message: String) -> PipelineWarning {
    PipelineWarning { level: WarningLevel::Error, message, event_id: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{browse, cmd, edit, focus, seq};

    fn ids(session: &[NormalizedEvent], out: &SessionClusters) -> Vec<Vec<String>> {
        out.clusters.iter().map(|c| c.iter().map(|&i| session[i].id.clone()).collect()).collect()
    }

    #[test]
    fn empty_and_single() {
        assert!(cluster_session(&[]).clusters.is_empty());
        let s = seq(vec![edit("a", 0, "/p/a.py", "/p")]);
        assert_eq!(cluster_session(&s).clusters, vec![vec![0]]);
    }

    #[test]
    fn gap_boundary_then_gap_merge_when_similar() {
        // Same file/project on both sides of a 6-minute gap: split, then re-joined.
        let s = seq(vec![
            edit("a", 0, "/p/a.py", "/p"),
            edit("b", 60, "/p/a.py", "/p"),
            edit("c", 60 + 301, "/p/a.py", "/p"),
        ]);
        let out = cluster_session(&s);
        assert_eq!(out.clusters, vec![vec![0, 1, 2]]);
    }

    #[test]
    fn gap_boundary_kept_when_dissimilar() {
        let s = seq(vec![
            edit("a", 0, "/p/a.py", "/p"),
            edit("b", 60, "/p/a.py", "/p"),
            edit("c", 60 + 301, "/q/b.py", "/q"),
        ]);
        let out = cluster_session(&s);
        assert_eq!(ids(&s, &out), vec![vec!["a", "b"], vec!["c"]]);
        assert_eq!(out.boundaries, vec![None, Some(BoundaryReason::Gap)]);
    }

    #[test]
    fn exactly_five_minutes_is_adjacent() {
        let s = seq(vec![edit("a", 0, "/p/a.py", "/p"), edit("b", 300, "/q/b.py", "/p")]);
        assert_eq!(cluster_session(&s).clusters.len(), 1);
    }

    #[test]
    fn command_phase_needs_two_meaningful_events() {
        // Focus events do not count as meaningful.
        let s = seq(vec![
            focus("f1", 0, "t"),
            focus("f2", 10, "t"),
            edit("a", 20, "/p/a.py", "/p"),
            cmd("c", 30, "pytest", "/p", 0),
        ]);
        assert_eq!(cluster_session(&s).clusters.len(), 1);

        let s =
            seq(vec![edit("a", 0, "/p/a.py", "/p"), edit("b", 10, "/p/a.py", "/p"), cmd("c", 20, "pytest", "/p", 1)]);
        let out = cluster_session(&s);
        assert_eq!(ids(&s, &out), vec![vec!["a", "b"], vec!["c"]]);
        assert_eq!(out.boundaries[1], Some(BoundaryReason::CommandPhase));
    }

    #[test]
    fn repeated_command_family_does_not_split() {
        let s = seq(vec![
            cmd("a", 0, "pytest", "/p", 1),
            edit("b", 10, "/p/a.py", "/p"),
            cmd("c", 20, "pytest", "/p", 1),
            cmd("d", 30, "pytest", "/p", 0),
        ]);
        assert_eq!(cluster_session(&s).clusters.len(), 1);
    }

    #[test]
    fn topic_shift_on_command_family_change() {
        // Only one meaningful event before, so no command phase; the command family
        // differs from the run's dominant one -> topic shift.
        let s = seq(vec![cmd("a", 0, "git status", "/p", 0), cmd("b", 10, "docker ps", "/p", 0)]);
        let out = cluster_session(&s);
        assert_eq!(out.clusters.len(), 2);
        assert_eq!(out.boundaries[1], Some(BoundaryReason::TopicShift));
    }

    #[test]
    fn topic_shift_on_project_and_file_change() {
        let s = seq(vec![edit("a", 0, "/p/a.py", "/p"), edit("b", 10, "/q/b.py", "/q")]);
        assert_eq!(cluster_session(&s).boundaries, vec![None, Some(BoundaryReason::TopicShift)]);
        // Project differs but file is the same name and no commands: no shift.
        let s = seq(vec![edit("a", 0, "/p/a.py", "/p"), edit("b", 10, "/q/a.py", "/q")]);
        assert_eq!(cluster_session(&s).clusters.len(), 1);
    }

    #[test]
    fn cap_folds_smallest_into_most_similar_neighbour() {
        // Six topic-shifted single-project runs alternating projects; cap to four.
        let mut events = Vec::new();
        let mut ts = 0;
        for (i, (file, ws)) in [
            ("/p/a.py", "/p"),
            ("/q/b.py", "/q"),
            ("/p/a.py", "/p"),
            ("/q/b.py", "/q"),
            ("/p/a.py", "/p"),
            ("/q/b.py", "/q"),
        ]
        .iter()
        .enumerate()
        {
            for k in 0..(i + 1) {
                events.push(edit(&format!("e{i}-{k}"), ts, file, ws));
                ts += 10;
            }
        }
        let s = seq(events);
        let out = cluster_session(&s);
        assert_eq!(out.clusters.len(), MAX_CLUSTERS_PER_SESSION);
        assert!(check_partition(&s, &out.clusters, true).is_empty());
        // Smallest run (1 event, index 0) has one neighbour; it folds right.
        assert_eq!(out.clusters[0].len(), 1 + 2 + 3);
    }

    #[test]
    fn cap_clusters_keeps_payload_in_step() {
        let s = seq((0..6).map(|i| browse(&format!("b{i}"), i * 10, &format!("https://site{i}.com/"))).collect());
        let clusters: Vec<Vec<usize>> = (0..6).map(|i| vec![i]).collect();
        let (clusters, payload) = cap_clusters(&s, clusters, (0..6).map(|i| vec![i]).collect(), 4, |mut a, b, _, _| {
            a.extend(b);
            a
        });
        assert_eq!(clusters.len(), 4);
        let flat: Vec<usize> = payload.iter().flatten().copied().collect();
        assert_eq!(flat, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(clusters, payload);
    }

    #[test]
    fn large_session_is_linear_enough() {
        // 0.1 recomputed the run topic per event (O(n²)); 20k events must be quick.
        let events: Vec<NormalizedEvent> = (0..20_000)
            .map(|i| {
                if i % 50 == 49 {
                    cmd(&format!("c{i}"), i, "git status", "/p", 0)
                } else {
                    edit(&format!("e{i}"), i, "/p/a.py", "/p")
                }
            })
            .collect();
        let s = seq(events);
        let started = std::time::Instant::now();
        let out = cluster_session(&s);
        assert!(out.clusters.len() <= MAX_CLUSTERS_PER_SESSION);
        assert!(check_partition(&s, &out.clusters, true).is_empty());
        // Worst case for the cap: thousands of alternating-project runs.
        let events: Vec<NormalizedEvent> = (0..8_000)
            .map(|i| {
                if (i / 2) % 2 == 0 {
                    edit(&format!("a{i}"), i, "/p/a.py", "/p")
                } else {
                    edit(&format!("b{i}"), i, "/q/b.py", "/q")
                }
            })
            .collect();
        let s = seq(events);
        let out = cluster_session(&s);
        assert_eq!(out.clusters.len(), MAX_CLUSTERS_PER_SESSION);
        assert!(check_partition(&s, &out.clusters, true).is_empty());
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    }

    #[test]
    fn similarity_weights() {
        let a = Topic::of(&seq(vec![cmd("a", 0, "pytest", "/p", 0)]));
        let b = Topic::of(&seq(vec![cmd("b", 0, "pytest", "/p", 0)]));
        assert!((topic_similarity(&a, &b) - 1.0).abs() < 1e-9);
        let c = Topic::of(&seq(vec![cmd("c", 0, "git", "/p", 0)]));
        // project (0.4) matches, command (0.3) differs, context terms (0.15) overlap ("/p" has none;
        // "pytest"/"git" tokens differ).
        let sim = topic_similarity(&a, &c);
        assert!((sim - 0.4 / 0.85).abs() < 1e-9, "{sim}");
        assert_eq!(topic_similarity(&Topic::default(), &Topic::default()), 0.0);
    }

    #[test]
    fn partition_checks_report_problems() {
        let s = seq(vec![edit("a", 0, "/p/a.py", "/p"), edit("b", 10, "/p/a.py", "/p")]);
        assert!(check_partition(&s, &[vec![0], vec![1]], true).is_empty());
        assert!(!check_partition(&s, &[vec![0]], false).is_empty());
        assert!(!check_partition(&s, &[vec![0, 0, 1]], false).is_empty());
        assert!(!check_partition(&s, &[vec![1], vec![0]], false).is_empty());
        assert!(!check_partition(&s, &[vec![], vec![0, 1]], false).is_empty());
        assert!(!check_partition(&s, &[vec![0, 1, 2]], false).is_empty());
    }
}
