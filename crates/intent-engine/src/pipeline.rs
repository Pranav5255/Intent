//! Day pipeline: normalize → sessionize → cluster → correlate → (refine) → cap →
//! label → enrich/resume → tree (1.x `pipeline.py`).
//!
//! No storage access: the daemon passes events in, checks its cache with
//! [`source_hash`], and persists the returned [`PipelineResult`].

use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use futures::stream::{self, StreamExt};
use intent_llm::{ClusterSemantics, Embedder, LabelLevel, LabelProvider, SemanticFallback, SemanticRefiner};
use intent_types::{
    ContextEvidence, DayExport, Intent, IntentInsights, NormalizedEvent, PipelineResult, PipelineWarning, RawEvent,
    SafeIntentFeatures, SemanticIntentMetadata, WarningLevel,
};
use uuid::Uuid;

use crate::cluster::{MAX_CLUSTERS_PER_SESSION, Topic, cap_clusters, check_partition, cluster_session};
use crate::correlate::{CorrelateConfig, merge_by_similarity};
use crate::enrich::{
    aggregate_stats, compute_insight_browser, compute_insight_editor, compute_insight_shell, compute_stats,
    derive_project_tag, detect_todos, validate_intent_tree,
};
use crate::labeling::{
    SAFE_FEATURE_POLICY_VERSION, TemplateLabelProvider, build_safe_cluster_features, build_safe_parent_features,
    label_with_fallback,
};
use crate::normalize::{compute_source_hash, normalize_events};
use crate::resume::{RESUME_POLICY_VERSION, build_resume_payload};
use crate::sessionize::{DEFAULT_SESSION_GAP_MINUTES, sessionize};
use crate::util::{sha256_hex16, truncate_chars, unique, with_timeout};

pub const PIPELINE_VERSION: &str = "2.0.0";
pub const MAX_INTENT_EVIDENCE: usize = 40;
pub const MAX_EVIDENCE_VALUE_CHARS: usize = 240;

#[derive(Debug, Clone, PartialEq)]
pub struct PipelineInput {
    pub date: String,
    pub events: Vec<RawEvent>,
}

impl From<DayExport> for PipelineInput {
    fn from(export: DayExport) -> Self {
        Self { date: export.date, events: export.events }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PipelineConfig {
    pub session_gap_minutes: i64,
    pub max_clusters_per_session: usize,
    /// Concurrent label calls across the whole run.
    pub label_concurrency: usize,
    pub label_timeout: Duration,
    /// Concurrent sessions in the clustering / refinement stage.
    pub session_concurrency: usize,
    pub semantic_timeout: Duration,
    pub correlate: CorrelateConfig,
    pub max_intent_evidence: usize,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            session_gap_minutes: DEFAULT_SESSION_GAP_MINUTES,
            max_clusters_per_session: MAX_CLUSTERS_PER_SESSION,
            label_concurrency: 4,
            label_timeout: Duration::from_secs(5),
            session_concurrency: 4,
            semantic_timeout: Duration::from_secs(60),
            correlate: CorrelateConfig::default(),
            max_intent_evidence: MAX_INTENT_EVIDENCE,
        }
    }
}

impl PipelineConfig {
    /// 1.x-equivalent behaviour (no correlation step); used by the parity tests.
    pub fn parity() -> Self {
        Self { correlate: CorrelateConfig::disabled(), ..Self::default() }
    }
}

#[derive(Clone)]
pub struct PipelineDeps {
    pub label_provider: Arc<dyn LabelProvider>,
    pub semantic_refiner: Option<Arc<dyn SemanticRefiner>>,
    /// Optional embedder for the correlation step (hashed TF-IDF otherwise).
    pub embedder: Option<Arc<dyn Embedder>>,
    pub fallback: TemplateLabelProvider,
    pub config: PipelineConfig,
}

impl PipelineDeps {
    pub fn new(label_provider: Arc<dyn LabelProvider>) -> Self {
        Self {
            label_provider,
            semantic_refiner: None,
            embedder: None,
            fallback: TemplateLabelProvider,
            config: PipelineConfig::default(),
        }
    }

    /// No inference at all: template labels, hashed correlation.
    pub fn template_only() -> Self {
        Self::new(Arc::new(TemplateLabelProvider))
    }
}

/// Everything that changes output besides the events: label provider, semantic
/// refiner, correlation mode and policy versions.
pub fn cache_identity(deps: &PipelineDeps) -> String {
    let mut identity = deps.label_provider.cache_identity();
    if let Some(refiner) = &deps.semantic_refiner {
        identity.push(':');
        identity.push_str(&refiner.cache_identity());
    }
    if deps.config.correlate.enabled {
        identity.push(':');
        identity.push_str(&deps.config.correlate.identity(deps.embedder.as_deref()));
    }
    identity
}

pub fn source_hash_for(normalized: &[NormalizedEvent], deps: &PipelineDeps) -> String {
    let input_hash = compute_source_hash(normalized);
    let c = &deps.config;
    sha256_hex16(
        format!(
            "{input_hash}:{}:{SAFE_FEATURE_POLICY_VERSION}:{RESUME_POLICY_VERSION}:pipeline-{PIPELINE_VERSION}:gap-{}:cap-{}:evidence-{}",
            cache_identity(deps),
            c.session_gap_minutes,
            c.max_clusters_per_session,
            c.max_intent_evidence
        )
        .as_bytes(),
    )
}

/// Cache key for `input` under `deps` (cheap; lets the daemon skip a run).
pub fn source_hash(input: &PipelineInput, deps: &PipelineDeps) -> String {
    source_hash_for(&normalize_events(&input.events).0, deps)
}

/// Stable id of an intent: the date, its scope (`session` or `cluster`) and the id
/// of its earliest event.
///
/// The scheduler re-runs the day on every idle period, and the app, the morning
/// notification, the copilot and stored embeddings keep ids across runs. So the id
/// must not depend on the day's source hash: events that arrive later, in another
/// session or as idle markers, leave it unchanged. Event ids are unique, a session's
/// clusters partition it, and the scope tells a session from its first cluster, so
/// ids are unique within a day. The id changes only when the intent gains an
/// earlier first event, for example when two clusters merge or a late event sorts
/// before it. A client that holds an old id then gets a 404.
pub fn intent_id(date: &str, scope: &str, first_event_id: &str) -> String {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, format!("intent/{date}/{scope}/{first_event_id}").as_bytes()).to_string()
}

fn session_id(date: &str, session: &[NormalizedEvent]) -> String {
    intent_id(date, "session", session.first().map_or("", |e| e.id.as_str()))
}

struct SessionPlan {
    range: Range<usize>,
    clusters: Vec<Vec<usize>>,
    semantics: Vec<Option<ClusterSemantics>>,
    refined_by: Option<String>,
    warnings: Vec<PipelineWarning>,
}

fn warning(level: WarningLevel, message: String) -> PipelineWarning {
    PipelineWarning { level, message, event_id: None }
}

fn fallback_name(reason: SemanticFallback) -> &'static str {
    match reason {
        SemanticFallback::Disabled => "disabled",
        SemanticFallback::ProviderUnavailable => "provider_unavailable",
        SemanticFallback::Timeout => "timeout",
        SemanticFallback::InvalidResponse => "invalid_response",
    }
}

/// Sort indices inside clusters and clusters by first event; then validate.
fn normalise_outcome(
    session: &[NormalizedEvent],
    mut clusters: Vec<Vec<usize>>,
    semantics: Vec<ClusterSemantics>,
) -> Result<(Vec<Vec<usize>>, Vec<ClusterSemantics>), String> {
    if clusters.len() != semantics.len() {
        return Err(format!("{} clusters but {} semantic records", clusters.len(), semantics.len()));
    }
    for c in &mut clusters {
        c.sort_unstable();
    }
    let mut paired: Vec<(Vec<usize>, ClusterSemantics)> = clusters.into_iter().zip(semantics).collect();
    paired.sort_by_key(|(c, _)| c.first().copied());
    let (clusters, semantics): (Vec<_>, Vec<_>) = paired.into_iter().unzip();
    let problems = check_partition(session, &clusters, false);
    if problems.is_empty() { Ok((clusters, semantics)) } else { Err(problems.join("; ")) }
}

fn merge_semantics(
    a: Option<ClusterSemantics>,
    b: Option<ClusterSemantics>,
    na: usize,
    nb: usize,
) -> Option<ClusterSemantics> {
    match (a, b) {
        (None, other) | (other, None) => other,
        (Some(a), Some(b)) => {
            let confidence = match (a.confidence, b.confidence) {
                (Some(x), Some(y)) => Some((x * na as f64 + y * nb as f64) / (na + nb).max(1) as f64),
                (x, y) => x.or(y),
            };
            let mut roles = a.roles.clone();
            for (role, count) in b.roles {
                *roles.entry(role).or_insert(0) += count;
            }
            let topic = if nb > na { b.topic.or(a.topic) } else { a.topic.or(b.topic) };
            Some(ClusterSemantics { confidence, roles, topic })
        }
    }
}

async fn plan_session(
    index: usize,
    range: Range<usize>,
    events: &[NormalizedEvent],
    deps: &PipelineDeps,
) -> SessionPlan {
    let session = &events[range.clone()];
    let deterministic = cluster_session(session);
    let mut warnings = deterministic.warnings;
    let mut clusters = deterministic.clusters;

    if deps.config.correlate.enabled {
        let correlated = merge_by_similarity(clusters, session, deps.embedder.as_deref(), &deps.config.correlate).await;
        warnings.extend(correlated.warnings);
        clusters = correlated.clusters;
    }

    let mut semantics: Vec<Option<ClusterSemantics>> = vec![None; clusters.len()];
    let mut refined_by = None;
    if let Some(refiner) = &deps.semantic_refiner {
        let outcome = with_timeout(refiner.refine(session, &clusters), deps.config.semantic_timeout).await;
        let fallback = match outcome {
            None => Some(SemanticFallback::Timeout),
            Some(Err(SemanticFallback::Disabled)) => None,
            Some(Err(reason)) => Some(reason),
            Some(Ok(outcome)) => match normalise_outcome(session, outcome.clusters, outcome.semantics) {
                Ok((refined, sem)) => {
                    clusters = refined;
                    semantics = sem.into_iter().map(Some).collect();
                    refined_by = Some(refiner.cache_identity());
                    None
                }
                Err(problem) => {
                    tracing::warn!(session = index, %problem, "semantic outcome rejected");
                    Some(SemanticFallback::InvalidResponse)
                }
            },
        };
        if let Some(reason) = fallback {
            warnings.push(warning(
                WarningLevel::Warning,
                format!("Semantic refinement fallback: {}", fallback_name(reason)),
            ));
        }
    }

    // 1.x applied no cap after semantic refinement; correlation never adds clusters.
    let (clusters, semantics) =
        cap_clusters(session, clusters, semantics, deps.config.max_clusters_per_session, merge_semantics);
    SessionPlan { range, clusters, semantics, refined_by, warnings }
}

/// Round-robin across sources so every event/child is represented, deduplicated,
/// with long values truncated.
fn capped_evidence<'a, I>(sources: I, max: usize) -> Vec<ContextEvidence>
where
    I: IntoIterator<Item = &'a [ContextEvidence]>,
{
    let lists: Vec<&[ContextEvidence]> = sources.into_iter().filter(|l| !l.is_empty()).collect();
    let mut out: Vec<ContextEvidence> = Vec::new();
    let mut depth = 0;
    while out.len() < max {
        let mut any = false;
        for list in &lists {
            let Some(item) = list.get(depth) else { continue };
            any = true;
            let value = truncate_chars(&item.value, MAX_EVIDENCE_VALUE_CHARS).to_owned();
            let candidate = ContextEvidence { field: item.field.clone(), value };
            if !out.contains(&candidate) {
                out.push(candidate);
                if out.len() >= max {
                    break;
                }
            }
        }
        if !any {
            break;
        }
        depth += 1;
    }
    out
}

fn cluster_prefix(cluster: &[&NormalizedEvent]) -> Option<(String, String, String)> {
    if cluster.len() < 3 {
        return None;
    }
    crate::prediction::event_prefix(&cluster[cluster.len() - 3..])
}

fn cluster_semantic(
    sem: &ClusterSemantics,
    cluster: &[&NormalizedEvent],
    provider: &str,
) -> Option<SemanticIntentMetadata> {
    if sem.confidence.is_none() && sem.roles.is_empty() && sem.topic.is_none() {
        return None;
    }
    let roots = unique(cluster.iter().flat_map(|e| e.entities.project_paths.iter().cloned()));
    Some(SemanticIntentMetadata {
        refined: true,
        confidence: sem.confidence,
        event_roles: sem.roles.clone(),
        topic: sem
            .topic
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty() && !t.eq_ignore_ascii_case("unknown"))
            .map(str::to_owned),
        workspace_root: if roots.len() == 1 { roots.into_iter().next() } else { None },
        provider_identity: Some(provider.to_owned()),
    })
}

fn aggregate_semantic(children: &[Intent]) -> Option<SemanticIntentMetadata> {
    let metadata: Vec<&SemanticIntentMetadata> = children.iter().filter_map(|c| c.semantic.as_ref()).collect();
    if metadata.is_empty() {
        return None;
    }
    let mut roles = std::collections::BTreeMap::new();
    for m in &metadata {
        for (role, count) in &m.event_roles {
            *roles.entry(role.clone()).or_insert(0) += count;
        }
    }
    let confidences: Vec<f64> = metadata.iter().filter_map(|m| m.confidence).collect();
    let single = |values: Vec<String>| {
        let values = unique(values);
        if values.len() == 1 { values.into_iter().next() } else { None }
    };
    Some(SemanticIntentMetadata {
        refined: true,
        confidence: (!confidences.is_empty()).then(|| confidences.iter().sum::<f64>() / confidences.len() as f64),
        event_roles: roles,
        topic: single(metadata.iter().filter_map(|m| m.topic.clone()).collect()),
        workspace_root: single(metadata.iter().filter_map(|m| m.workspace_root.clone()).collect()),
        provider_identity: single(metadata.iter().filter_map(|m| m.provider_identity.clone()).collect()),
    })
}

struct ChildDraft {
    intent: Intent,
    features: SafeIntentFeatures,
    command_family: Option<String>,
}

fn build_child(
    date: &str,
    parent_id: &str,
    plan: &SessionPlan,
    j: usize,
    cluster: &[&NormalizedEvent],
    max_evidence: usize,
) -> ChildDraft {
    let semantic = match (&plan.semantics[j], &plan.refined_by) {
        (Some(sem), Some(provider)) => cluster_semantic(sem, cluster, provider),
        _ => None,
    };
    let tag = derive_project_tag(cluster);
    let features =
        build_safe_cluster_features(cluster, semantic.as_ref().and_then(|s| s.topic.as_deref()), tag.as_deref());
    let intent = Intent {
        id: intent_id(date, "cluster", &cluster[0].id),
        parent_id: Some(parent_id.to_owned()),
        date: date.to_owned(),
        label: "Work Task".into(),
        summary: "Inferred work session".into(),
        confidence: 0.0,
        start_ts: cluster[0].ts,
        end_ts: cluster[cluster.len() - 1].ts,
        depth: 1,
        tags: tag.into_iter().collect(),
        stats: compute_stats(cluster),
        insights: IntentInsights {
            editor: compute_insight_editor(cluster),
            browser: compute_insight_browser(cluster),
            shell: compute_insight_shell(cluster),
        },
        todos: detect_todos(cluster),
        evidence: capped_evidence(cluster.iter().map(|e| e.evidence.as_slice()), max_evidence),
        resume_payload: build_resume_payload(cluster),
        prefix: cluster_prefix(cluster),
        semantic,
        privacy_policy_version: SAFE_FEATURE_POLICY_VERSION.into(),
        children: Vec::new(),
    };
    ChildDraft {
        intent,
        features,
        command_family: Topic::of(cluster.iter().copied()).command_family().map(str::to_owned),
    }
}

fn build_parent(date: &str, session: &[NormalizedEvent], children: Vec<Intent>, max_evidence: usize) -> Intent {
    let all: Vec<&NormalizedEvent> = session.iter().collect();
    Intent {
        id: session_id(date, session),
        parent_id: None,
        date: date.to_owned(),
        label: "Work Session".into(),
        summary: "Inferred work session".into(),
        confidence: 0.0,
        start_ts: session[0].ts,
        end_ts: session[session.len() - 1].ts,
        depth: 0,
        tags: unique(children.iter().flat_map(|c| c.tags.iter().cloned())),
        stats: aggregate_stats(&children),
        insights: IntentInsights::default(),
        todos: Vec::new(),
        evidence: capped_evidence(children.iter().map(|c| c.evidence.as_slice()), max_evidence),
        // The parent is the "open this session" action: rebuild from the whole
        // chronology so each tab keeps its final URL.
        resume_payload: build_resume_payload(&all),
        prefix: None,
        semantic: aggregate_semantic(&children),
        privacy_policy_version: SAFE_FEATURE_POLICY_VERSION.into(),
        children,
    }
}

/// Run the full pipeline for one day.
pub async fn run(input: PipelineInput, deps: &PipelineDeps) -> PipelineResult {
    let config = &deps.config;
    let (events, mut warnings) = normalize_events(&input.events);
    drop(input.events);
    let hash = source_hash_for(&events, deps);
    let date = input.date;

    // Stage 1: cluster / correlate / refine sessions concurrently.
    let sessions = sessionize(&events, config.session_gap_minutes);
    // Eager + boxed for the same `Send` reason as in `label_all`.
    let session_futures: Vec<BoxFuture<'_, SessionPlan>> = sessions
        .into_iter()
        .enumerate()
        .map(|(index, range)| -> BoxFuture<'_, SessionPlan> { Box::pin(plan_session(index, range, &events, deps)) })
        .collect();
    let plans: Vec<SessionPlan> =
        stream::iter(session_futures).buffered(config.session_concurrency.max(1)).collect().await;

    // Stage 2: build children and label every cluster concurrently (bounded).
    let mut drafts: Vec<Vec<ChildDraft>> = Vec::with_capacity(plans.len());
    for plan in &plans {
        warnings.extend(plan.warnings.iter().cloned());
        let session = &events[plan.range.clone()];
        let parent_id = session_id(&date, session);
        drafts.push(
            plan.clusters
                .iter()
                .enumerate()
                .map(|(j, indices)| {
                    let cluster: Vec<&NormalizedEvent> = indices.iter().map(|&i| &session[i]).collect();
                    build_child(&date, &parent_id, plan, j, &cluster, config.max_intent_evidence)
                })
                .collect(),
        );
    }
    let jobs: Vec<&SafeIntentFeatures> = drafts.iter().flatten().map(|d| &d.features).collect();
    let mut fallbacks = 0usize;
    let labels = label_all(deps, jobs, LabelLevel::Cluster, &mut fallbacks).await;
    let mut labels = labels.into_iter();

    // Stage 3: assemble trees; label multi-cluster parents concurrently.
    let mut roots: Vec<Option<Intent>> = Vec::with_capacity(plans.len());
    let mut parents: Vec<(usize, Intent, SafeIntentFeatures)> = Vec::new();
    for (plan, plan_drafts) in plans.iter().zip(drafts) {
        let mut families = Vec::new();
        let mut children = Vec::with_capacity(plan_drafts.len());
        for draft in plan_drafts {
            let mut child = draft.intent;
            if let Some(label) = labels.next() {
                child.label = label.label;
                child.summary = label.summary;
                child.confidence = label.confidence;
            }
            families.extend(draft.command_family);
            children.push(child);
        }
        match children.len() {
            0 => roots.push(None),
            1 => {
                // A one-task session takes the session id, so the root keeps its id
                // when later work in the session adds a second task.
                let mut only = children.remove(0);
                only.id = only.parent_id.take().unwrap_or(only.id);
                only.depth = 0;
                roots.push(Some(only));
            }
            _ => {
                let session = &events[plan.range.clone()];
                let parent = build_parent(&date, session, children, config.max_intent_evidence);
                for problem in validate_intent_tree(&parent) {
                    warnings.push(warning(WarningLevel::Error, format!("Intent tree invariant violated: {problem}")));
                }
                let features =
                    build_safe_parent_features(&parent.children, &families, parent.tags.first().map(String::as_str));
                parents.push((roots.len(), parent, features));
                roots.push(None);
            }
        }
    }
    let parent_labels =
        label_all(deps, parents.iter().map(|(_, _, f)| f).collect(), LabelLevel::Parent, &mut fallbacks).await;
    for ((slot, mut parent, _), label) in parents.into_iter().zip(parent_labels) {
        parent.label = label.label;
        parent.summary = label.summary;
        parent.confidence = label.confidence;
        roots[slot] = Some(parent);
    }
    if fallbacks > 0 {
        warnings.push(warning(
            WarningLevel::Warning,
            format!("{fallbacks} label(s) used the template fallback ({})", deps.label_provider.cache_identity()),
        ));
    }

    PipelineResult {
        intents: roots.into_iter().flatten().collect(),
        warnings,
        source_hash: hash,
        pipeline_version: PIPELINE_VERSION.into(),
        cached: false,
    }
}

async fn label_all(
    deps: &PipelineDeps,
    jobs: Vec<&SafeIntentFeatures>,
    level: LabelLevel,
    fallbacks: &mut usize,
) -> Vec<intent_llm::LabelResult> {
    let provider = deps.label_provider.as_ref();
    // The futures are built eagerly and boxed as `Send`: a borrowing `.map(closure)`
    // stream held across `.await` makes rustc unable to prove `run()`'s future is `Send`
    // for every lifetime, so it could not be spawned on a multi-threaded runtime.
    let futures: Vec<BoxFuture<'_, _>> = jobs
        .into_iter()
        .map(|features| -> BoxFuture<'_, _> {
            Box::pin(label_with_fallback(provider, &deps.fallback, features, level, deps.config.label_timeout))
        })
        .collect();
    let results: Vec<_> = stream::iter(futures).buffered(deps.config.label_concurrency.max(1)).collect().await;
    results
        .into_iter()
        .map(|(label, reason)| {
            if reason.is_some() {
                *fallbacks += 1;
            }
            label
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::raw;
    use async_trait::async_trait;
    use intent_llm::{LabelResult, LlmError, SemanticOutcome};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn day() -> PipelineInput {
        let mut events = Vec::new();
        let mut ts = 1_000_000;
        for i in 0..4 {
            events.push(raw(
                &format!("e{i}"),
                ts,
                "vscode",
                "document_change",
                json!({"path": "/p/a.py", "workspace": "/p"}),
            ));
            ts += 30;
        }
        events.push(raw("c0", ts, "shell", "command", json!({"cmd": "pytest", "cwd": "/p", "exit_code": 1})));
        ts += 30;
        events.push(raw("c1", ts, "shell", "command", json!({"cmd": "pytest", "cwd": "/p", "exit_code": 0})));
        ts += 3600; // new session
        events.push(raw("b0", ts, "firefox", "tab_change", json!({"url": "https://docs.rs/x", "tab_id": 1})));
        PipelineInput { date: "2026-11-14".into(), events }
    }

    fn parity_deps() -> PipelineDeps {
        PipelineDeps { config: PipelineConfig::parity(), ..PipelineDeps::template_only() }
    }

    #[tokio::test]
    async fn builds_tree_with_single_cluster_roots() {
        let deps = parity_deps();
        let result = run(day(), &deps).await;
        assert_eq!(result.pipeline_version, "2.0.0");
        assert_eq!(result.intents.len(), 2);
        let parent = &result.intents[0];
        assert_eq!(parent.depth, 0);
        assert_eq!(parent.children.len(), 2);
        assert_eq!(parent.label, "Work on p");
        assert_eq!(parent.children[0].label, "Edit a.py");
        assert_eq!(parent.children[1].label, "Run Pytest");
        assert!(parent.children.iter().all(|c| c.parent_id.as_deref() == Some(parent.id.as_str())));
        assert_eq!(parent.id, intent_id("2026-11-14", "session", "e0"));
        assert_eq!(parent.children[1].id, intent_id("2026-11-14", "cluster", "c0"));
        let single = &result.intents[1];
        assert_eq!(single.depth, 0);
        assert!(single.parent_id.is_none());
        assert_eq!(single.id, intent_id("2026-11-14", "session", "b0"));
        assert_eq!(single.label, "Research docs.rs");
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
        assert_eq!(source_hash(&day(), &deps), result.source_hash);
    }

    fn all_ids(intents: &[Intent]) -> Vec<String> {
        intents.iter().flat_map(|i| std::iter::once(i.id.clone()).chain(all_ids(&i.children))).collect()
    }

    #[tokio::test]
    async fn ids_survive_an_idle_period() {
        // The scheduler re-runs the day a few minutes into every idle period. The
        // idle markers change the source hash but must not change any intent id.
        let deps = parity_deps();
        let before = run(day(), &deps).await;
        let mut input = day();
        let last = input.events.iter().map(|e| e.ts).max().unwrap_or_default();
        input.events.push(raw("idle-start", last + 60, "linux", "idle_start", json!({})));
        input.events.push(raw("idle-end", last + 900, "linux", "idle_end", json!({})));
        let after = run(input, &deps).await;
        assert_ne!(before.source_hash, after.source_hash);
        assert_eq!(all_ids(&before.intents), all_ids(&after.intents));
    }

    #[tokio::test]
    async fn later_work_keeps_the_ids_of_earlier_intents() {
        let deps = parity_deps();
        let before = run(day(), &deps).await;
        let mut input = day();
        let last = input.events.iter().map(|e| e.ts).max().unwrap_or_default();
        // Grows the second session and starts a third one.
        input.events.push(raw(
            "b1",
            last + 30,
            "firefox",
            "tab_change",
            json!({"url": "https://docs.rs/y", "tab_id": 1}),
        ));
        input.events.push(raw("late", last + 7200, "shell", "command", json!({"cmd": "cargo test", "cwd": "/q"})));
        let after = run(input, &deps).await;
        assert_eq!(after.intents.len(), 3);
        assert_eq!(all_ids(&before.intents[..1]), all_ids(&after.intents[..1]));
        assert_eq!(before.intents[1].id, after.intents[1].id);
    }

    #[tokio::test]
    async fn a_root_keeps_its_id_when_its_session_splits() {
        // A one-task session becomes a parent with two tasks when different work
        // follows after a short gap; the root a drawer or a notification points at stays the same.
        let deps = parity_deps();
        let before = run(day(), &deps).await;
        let mut input = day();
        let last = input.events.iter().map(|e| e.ts).max().unwrap_or_default();
        for i in 0..2 {
            input.events.push(raw(
                &format!("x{i}"),
                last + 400 + 30 * i,
                "shell",
                "command",
                json!({"cmd": "git status", "cwd": "/other"}),
            ));
        }
        let after = run(input, &deps).await;
        assert_eq!(after.intents[1].children.len(), 2, "{:?}", after.intents[1]);
        assert_eq!(before.intents[1].id, after.intents[1].id);
    }

    #[tokio::test]
    async fn ids_are_unique_within_a_day() {
        let mut deps = parity_deps();
        deps.semantic_refiner = Some(Arc::new(Refiner(Ok(SemanticOutcome { clusters: vec![], semantics: vec![] }))));
        for input in [day(), many_sessions()] {
            for result in [run(input.clone(), &parity_deps()).await, run(input, &deps).await] {
                let ids = all_ids(&result.intents);
                let distinct: std::collections::HashSet<&String> = ids.iter().collect();
                assert_eq!(distinct.len(), ids.len(), "{ids:?}");
            }
        }
    }

    #[tokio::test]
    async fn hash_depends_on_provider_and_correlation() {
        let a = source_hash(&day(), &parity_deps());
        let b = source_hash(&day(), &PipelineDeps::template_only());
        assert_ne!(a, b);
        let mut deps = parity_deps();
        deps.label_provider = Arc::new(Slow {
            delay: Duration::ZERO,
            inflight: Default::default(),
            max: Default::default(),
            fail: false,
        });
        assert_ne!(a, source_hash(&day(), &deps));
    }

    struct Slow {
        delay: Duration,
        inflight: AtomicUsize,
        max: AtomicUsize,
        fail: bool,
    }

    #[async_trait]
    impl LabelProvider for Slow {
        fn cache_identity(&self) -> String {
            "slow-test".into()
        }
        async fn label(&self, _f: &SafeIntentFeatures, _l: LabelLevel) -> Result<LabelResult, LlmError> {
            let now = self.inflight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max.fetch_max(now, Ordering::SeqCst);
            futures_timer::Delay::new(self.delay).await;
            self.inflight.fetch_sub(1, Ordering::SeqCst);
            if self.fail {
                return Err(LlmError::new("down"));
            }
            Ok(LabelResult { label: "LLM Label".into(), summary: "From the model.".into(), confidence: 0.9 })
        }
    }

    fn many_sessions() -> PipelineInput {
        let mut events = Vec::new();
        for s in 0..6 {
            let base = 2_000_000 + s * 7200;
            events.push(raw(
                &format!("s{s}a"),
                base,
                "vscode",
                "document_change",
                json!({"path": format!("/p{s}/a.py"), "workspace": format!("/p{s}")}),
            ));
            events.push(raw(
                &format!("s{s}b"),
                base + 10,
                "vscode",
                "document_change",
                json!({"path": format!("/p{s}/a.py"), "workspace": format!("/p{s}")}),
            ));
            events.push(raw(
                &format!("s{s}c"),
                base + 20,
                "shell",
                "command",
                json!({"cmd": "git status", "cwd": format!("/p{s}")}),
            ));
        }
        PipelineInput { date: "d".into(), events }
    }

    #[tokio::test]
    async fn labels_run_concurrently_but_bounded() {
        let provider = Arc::new(Slow {
            delay: Duration::from_millis(40),
            inflight: Default::default(),
            max: Default::default(),
            fail: false,
        });
        let mut deps = parity_deps();
        deps.label_provider = provider.clone();
        let result = run(many_sessions(), &deps).await;
        // The in-flight high-water mark proves concurrency and its bound; a wall-clock
        // check failed on a loaded macOS runner (615 ms), so the test asserts no duration.
        let max = provider.max.load(Ordering::SeqCst);
        assert!(max > 1 && max <= 4, "max in flight {max}");
        assert!(result.intents.iter().all(|i| i.label == "LLM Label"));
    }

    #[tokio::test]
    async fn timeouts_and_errors_fall_back_to_template() {
        let mut deps = parity_deps();
        deps.config.label_timeout = Duration::from_millis(10);
        deps.label_provider = Arc::new(Slow {
            delay: Duration::from_millis(200),
            inflight: Default::default(),
            max: Default::default(),
            fail: false,
        });
        let result = run(day(), &deps).await;
        assert_eq!(result.intents[0].label, "Work on p");
        assert!(result.warnings.iter().any(|w| w.message.contains("template fallback")));

        deps.label_provider =
            Arc::new(Slow { delay: Duration::ZERO, inflight: Default::default(), max: Default::default(), fail: true });
        let result = run(day(), &deps).await;
        assert_eq!(result.intents[0].children[1].label, "Run Pytest");
    }

    struct Refiner(Result<SemanticOutcome, SemanticFallback>);

    #[async_trait]
    impl SemanticRefiner for Refiner {
        fn cache_identity(&self) -> String {
            "refiner-test".into()
        }
        async fn refine(
            &self,
            session: &[NormalizedEvent],
            _d: &[Vec<usize>],
        ) -> Result<SemanticOutcome, SemanticFallback> {
            match &self.0 {
                Ok(o) if o.clusters.is_empty() => {
                    // One cluster per event: exercises the post-refinement cap.
                    Ok(SemanticOutcome {
                        clusters: (0..session.len()).map(|i| vec![i]).collect(),
                        semantics: (0..session.len())
                            .map(|i| ClusterSemantics {
                                confidence: Some(0.5 + i as f64 / 100.0),
                                roles: [("task".to_owned(), 1)].into(),
                                topic: Some("refunds".into()),
                            })
                            .collect(),
                    })
                }
                other => other.clone(),
            }
        }
    }

    #[tokio::test]
    async fn semantic_refinement_is_capped_and_annotated() {
        let mut deps = parity_deps();
        deps.semantic_refiner = Some(Arc::new(Refiner(Ok(SemanticOutcome { clusters: vec![], semantics: vec![] }))));
        let result = run(day(), &deps).await;
        let parent = &result.intents[0];
        assert_eq!(parent.children.len(), MAX_CLUSTERS_PER_SESSION);
        assert_eq!(parent.stats.event_count, 6);
        let sem = parent.semantic.as_ref().unwrap();
        assert_eq!(sem.topic.as_deref(), Some("refunds"));
        assert_eq!(sem.event_roles["task"], 6);
        assert_eq!(sem.workspace_root.as_deref(), Some("/p"));
        assert_eq!(sem.provider_identity.as_deref(), Some("refiner-test"));
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    }

    #[tokio::test]
    async fn invalid_or_failed_refinement_falls_back() {
        let mut deps = parity_deps();
        let baseline = run(day(), &deps).await;
        let bad = SemanticOutcome { clusters: vec![vec![0, 0]], semantics: vec![ClusterSemantics::default()] };
        deps.semantic_refiner = Some(Arc::new(Refiner(Ok(bad))));
        let result = run(day(), &deps).await;
        assert!(result.warnings.iter().any(|w| w.message == "Semantic refinement fallback: invalid_response"));
        assert_eq!(result.intents[0].children.len(), baseline.intents[0].children.len());
        assert!(result.intents[0].semantic.is_none());

        deps.semantic_refiner = Some(Arc::new(Refiner(Err(SemanticFallback::Disabled))));
        assert!(run(day(), &deps).await.warnings.is_empty());
        deps.semantic_refiner = Some(Arc::new(Refiner(Err(SemanticFallback::Timeout))));
        assert!(run(day(), &deps).await.warnings.iter().any(|w| w.message.ends_with("timeout")));
    }

    #[test]
    fn evidence_is_round_robin_capped_and_truncated() {
        let a: Vec<ContextEvidence> =
            (0..50).map(|i| ContextEvidence { field: "a".into(), value: format!("{i}") }).collect();
        let b = vec![ContextEvidence { field: "b".into(), value: "x".repeat(1000) }];
        let out = capped_evidence([a.as_slice(), b.as_slice()], 40);
        assert_eq!(out.len(), 40);
        assert_eq!(out[1].field, "b");
        assert_eq!(out[1].value.len(), MAX_EVIDENCE_VALUE_CHARS);
    }

    #[tokio::test]
    async fn run_future_can_be_spawned() {
        // Regression: the daemon runs the pipeline from axum handlers / tokio::spawn,
        // which require a `Send + 'static` future.
        let deps = Arc::new(PipelineDeps::template_only());
        let handle = tokio::spawn(async move { run(day(), &deps).await });
        assert!(!handle.await.unwrap().intents.is_empty());
    }

    #[test]
    fn uuid5_scheme() {
        // Python: str(uuid5(NAMESPACE_URL, "intent/2026-07-30/abc/session:0"))
        assert_eq!(intent_id("2026-07-30", "abc", "session:0"), "1cd3084a-c7b6-5f35-9c8b-132ba235ee29");
        assert_ne!(intent_id("d", "session", "e"), intent_id("d", "cluster", "e"));
        assert_eq!(intent_id("d", "h", "s"), intent_id("d", "h", "s"));
    }
}
