//! Stored-intent selection and resume preview (0.1 `resume_select.py`).
//!
//! Pure: the caller supplies the stored intents. Deterministic keyword scoring,
//! optionally reranked by a [`Reranker`]; a rerank is only accepted when it is an
//! exact permutation of the supplied ids. Never restores anything.

use async_trait::async_trait;
use intent_llm::LlmError;
use intent_types::{
    Intent, RestoreScope, ResumePayload, ResumeSelectCandidate, ResumeSelectPreview, ResumeSelectRequest,
    ResumeSelectResponse, ShellResume,
};

use crate::util::with_timeout;

pub const MAX_CANDIDATES: usize = 10;
/// Upper bound on candidates sent to a reranker.
pub const MAX_RERANK_CANDIDATES: usize = 20;
const QUERY_STOP_WORDS: &[&str] = &["a", "an", "and", "for", "only", "project", "resume", "the", "this", "to"];

/// What a reranker sees about each candidate (no evidence, no payload).
#[derive(Debug, Clone, PartialEq)]
pub struct RerankCandidate {
    pub intent_id: String,
    pub label: String,
    pub summary: String,
    pub tags: Vec<String>,
}

/// Orders known candidates for a query; must return every supplied id exactly once.
#[async_trait]
pub trait Reranker: Send + Sync {
    async fn rerank(&self, query: &str, candidates: &[RerankCandidate]) -> Result<Vec<String>, LlmError>;
}

#[derive(Debug, Clone, Copy)]
pub struct RerankOptions {
    pub timeout: std::time::Duration,
}

impl Default for RerankOptions {
    fn default() -> Self {
        Self { timeout: std::time::Duration::from_secs(8) }
    }
}

/// Resolve a resume request against `intents` (root intents; with `intent_id` set,
/// any node of the supplied trees is matched).
pub async fn select_resume_preview(
    intents: &[Intent],
    request: &ResumeSelectRequest,
    reranker: Option<&dyn Reranker>,
    options: RerankOptions,
) -> Option<ResumeSelectResponse> {
    let mut pool: Vec<&Intent> = match request.intent_id.as_deref().map(str::trim) {
        Some(id) if !id.is_empty() => vec![intents.iter().flat_map(Intent::walk).find(|i| i.id == id)?],
        _ => intents.iter().collect(),
    };
    if let Some(tag) = request.project_tag.as_deref().filter(|t| !t.trim().is_empty()) {
        let expected = normalized_project_tag(tag);
        pool.retain(|i| i.tags.iter().any(|t| normalized_project_tag(t) == expected));
    }
    let query = request.query.as_deref().filter(|q| !q.trim().is_empty());
    let mut ranked = rank_deterministically(pool, query);
    if let (Some(query), Some(reranker)) = (query, reranker) {
        ranked = rerank_known(ranked, query, reranker, options).await;
    }
    ranked.truncate(MAX_CANDIDATES);
    if ranked.is_empty() {
        return None;
    }
    let candidates: Vec<ResumeSelectCandidate> = ranked.iter().map(|(i, s)| candidate(i, *s)).collect();
    let Some(index) = selected_index(request, &ranked) else {
        return Some(ResumeSelectResponse { needs_picker: true, candidates, selected: None });
    };
    let (intent, score) = ranked[index];
    Some(ResumeSelectResponse {
        needs_picker: false,
        candidates,
        selected: Some(preview(intent, score, request.restore_scope)),
    })
}

fn query_terms(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty() && !QUERY_STOP_WORDS.contains(t))
        .map(str::to_owned)
        .collect()
}

/// Keyword score = occurrences of query terms in label + summary + tags.
pub fn rank_deterministically<'a>(intents: Vec<&'a Intent>, query: Option<&str>) -> Vec<(&'a Intent, f64)> {
    let terms = query.map(query_terms).unwrap_or_default();
    if terms.is_empty() {
        return intents.into_iter().map(|i| (i, 1.0)).collect();
    }
    let mut scored: Vec<(&Intent, f64)> = intents
        .into_iter()
        .filter_map(|intent| {
            let haystack = std::iter::once(intent.label.as_str())
                .chain(std::iter::once(intent.summary.as_str()))
                .chain(intent.tags.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            let score = terms.iter().map(|t| haystack.matches(t.as_str()).count()).sum::<usize>() as f64;
            (score > 0.0).then_some((intent, score))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.0.end_ts.cmp(&a.0.end_ts))
            .then(a.0.id.cmp(&b.0.id))
    });
    scored
}

async fn rerank_known<'a>(
    ranked: Vec<(&'a Intent, f64)>,
    query: &str,
    reranker: &dyn Reranker,
    options: RerankOptions,
) -> Vec<(&'a Intent, f64)> {
    if ranked.len() < 2 {
        return ranked;
    }
    let head = ranked.len().min(MAX_RERANK_CANDIDATES);
    let supplied: Vec<RerankCandidate> = ranked[..head]
        .iter()
        .map(|(i, _)| RerankCandidate {
            intent_id: i.id.clone(),
            label: i.label.clone(),
            summary: i.summary.clone(),
            tags: i.tags.clone(),
        })
        .collect();
    let order = match with_timeout(reranker.rerank(query, &supplied), options.timeout).await {
        Some(Ok(order)) => order,
        Some(Err(e)) => {
            tracing::warn!(error = %e, "resume rerank failed; keeping deterministic order");
            return ranked;
        }
        None => {
            tracing::warn!("resume rerank timed out; keeping deterministic order");
            return ranked;
        }
    };
    if !is_permutation(&order, &supplied) {
        tracing::warn!("resume rerank was not a permutation of the supplied ids; ignored");
        return ranked;
    }
    let mut out: Vec<(&Intent, f64)> =
        order.iter().filter_map(|id| ranked[..head].iter().find(|(i, _)| &i.id == id).copied()).collect();
    out.extend_from_slice(&ranked[head..]);
    out
}

/// `order` contains every supplied id exactly once and nothing else.
pub fn is_permutation(order: &[String], supplied: &[RerankCandidate]) -> bool {
    let mut a: Vec<&str> = order.iter().map(String::as_str).collect();
    let mut b: Vec<&str> = supplied.iter().map(|c| c.intent_id.as_str()).collect();
    a.sort_unstable();
    b.sort_unstable();
    a == b && a.windows(2).all(|w| w[0] != w[1])
}

fn selected_index(request: &ResumeSelectRequest, ranked: &[(&Intent, f64)]) -> Option<usize> {
    let set = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.trim().is_empty());
    if set(&request.intent_id) || set(&request.project_tag) {
        return (ranked.len() == 1).then_some(0);
    }
    if ranked.len() == 1 {
        return Some(0);
    }
    (ranked[0].1 >= 2.0 * ranked[1].1).then_some(0)
}

fn candidate(intent: &Intent, score: f64) -> ResumeSelectCandidate {
    ResumeSelectCandidate {
        intent_id: intent.id.clone(),
        label: intent.label.clone(),
        summary: intent.summary.clone(),
        project_tag: intent.tags.iter().find(|t| t.to_lowercase().starts_with("project:")).cloned(),
        workspace_root: workspace_root(intent),
        score,
    }
}

fn preview(intent: &Intent, score: f64, scope: Option<RestoreScope>) -> ResumeSelectPreview {
    let root = workspace_root(intent);
    let payload = match scope {
        Some(RestoreScope::SameProject) => scope_payload(&intent.resume_payload, root.as_deref()),
        None => intent.resume_payload.clone(),
    };
    ResumeSelectPreview { candidate: candidate(intent, score), resume_payload: payload }
}

fn normalized_project_tag(value: &str) -> String {
    let normalized = value.trim().to_lowercase();
    if normalized.starts_with("project:") { normalized } else { format!("project:{normalized}") }
}

/// Semantic workspace root, else the shell cwd if it contains every file, else the
/// single common parent of the files.
pub fn workspace_root(intent: &Intent) -> Option<String> {
    if let Some(root) = intent.semantic.as_ref().and_then(|s| s.workspace_root.clone()) {
        return Some(root);
    }
    let files = &intent.resume_payload.files;
    if let Some(cwd) = intent.resume_payload.shell.cwd.as_deref().filter(|c| !c.is_empty())
        && files.iter().all(|f| is_within(f, cwd))
    {
        return Some(cwd.to_owned());
    }
    if files.is_empty() {
        return None;
    }
    let parents: Vec<String> = crate::util::unique(files.iter().map(|f| pure_parent(f)));
    (parents.len() == 1).then(|| parents[0].clone())
}

fn scope_payload(payload: &ResumePayload, root: Option<&str>) -> ResumePayload {
    let Some(root) = root else {
        return ResumePayload { urls: payload.urls.clone(), ..Default::default() };
    };
    let shell = match payload.shell.cwd.as_deref() {
        Some(cwd) if is_within(cwd, root) => payload.shell.clone(),
        _ => ShellResume::default(),
    };
    ResumePayload {
        files: payload.files.iter().filter(|f| is_within(f, root)).cloned().collect(),
        urls: payload.urls.clone(),
        shell,
    }
}

fn is_within(path: &str, root: &str) -> bool {
    let norm = |p: &str| p.replace('\\', "/").trim_end_matches('/').to_lowercase();
    let (value, base) = (norm(path), norm(root));
    value == base || value.starts_with(&format!("{base}/"))
}

/// `PurePath(path).parent` for POSIX-style paths.
fn pure_parent(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let trimmed = if normalized.len() > 1 { normalized.trim_end_matches('/') } else { normalized.as_str() };
    match trimmed.rsplit_once('/') {
        Some(("", _)) => "/".into(),
        Some((parent, _)) => parent.into(),
        None => ".".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::intent;
    use intent_types::SemanticIntentMetadata;

    fn tagged(id: &str, label: &str, tag: &str, end: i64) -> Intent {
        let mut i = intent(id, label, 0, end);
        i.tags = vec![format!("project:{tag}")];
        i.resume_payload.files = vec![format!("/home/u/{tag}/src/a.rs")];
        i.resume_payload.shell =
            ShellResume { cwd: Some(format!("/home/u/{tag}")), last_cmd: Some("cargo test".into()) };
        i.resume_payload.urls = vec!["https://docs.rs/".into()];
        i
    }

    fn req(query: Option<&str>) -> ResumeSelectRequest {
        ResumeSelectRequest { query: query.map(str::to_owned), ..Default::default() }
    }

    struct Fixed(Result<Vec<String>, ()>);
    #[async_trait]
    impl Reranker for Fixed {
        async fn rerank(&self, _q: &str, _c: &[RerankCandidate]) -> Result<Vec<String>, LlmError> {
            self.0.clone().map_err(|()| LlmError::new("x"))
        }
    }

    #[tokio::test]
    async fn keyword_scoring_selects_clear_winner() {
        let intents = vec![tagged("a", "Fix ledger overdraft", "ledger", 10), tagged("b", "Write docs", "site", 20)];
        let r =
            select_resume_preview(&intents, &req(Some("resume the ledger overdraft")), None, RerankOptions::default())
                .await
                .unwrap();
        assert!(!r.needs_picker);
        let selected = r.selected.unwrap();
        assert_eq!(selected.candidate.intent_id, "a");
        assert_eq!(selected.candidate.workspace_root.as_deref(), Some("/home/u/ledger"));
        assert_eq!(r.candidates.len(), 1);
    }

    #[tokio::test]
    async fn ambiguous_needs_picker_and_no_match_is_none() {
        let intents = vec![tagged("a", "Fix ledger", "ledger", 10), tagged("b", "Fix site", "site", 20)];
        let r = select_resume_preview(&intents, &req(Some("fix")), None, RerankOptions::default()).await.unwrap();
        assert!(r.needs_picker);
        assert_eq!(r.candidates[0].intent_id, "b"); // tie broken by newest end_ts
        assert!(select_resume_preview(&intents, &req(Some("zebra")), None, RerankOptions::default()).await.is_none());
    }

    #[tokio::test]
    async fn rerank_must_be_a_permutation() {
        let intents = vec![tagged("a", "Fix ledger", "ledger", 10), tagged("b", "Fix site", "site", 20)];
        let good = Fixed(Ok(vec!["a".into(), "b".into()]));
        let r =
            select_resume_preview(&intents, &req(Some("fix")), Some(&good), RerankOptions::default()).await.unwrap();
        assert_eq!(r.candidates[0].intent_id, "a");
        for bad in [
            Fixed(Ok(vec!["a".into()])),
            Fixed(Ok(vec!["a".into(), "a".into()])),
            Fixed(Ok(vec!["a".into(), "zzz".into()])),
            Fixed(Err(())),
        ] {
            let r =
                select_resume_preview(&intents, &req(Some("fix")), Some(&bad), RerankOptions::default()).await.unwrap();
            assert_eq!(r.candidates[0].intent_id, "b");
        }
    }

    #[tokio::test]
    async fn exact_id_project_filter_and_scope() {
        let mut root = tagged("a", "Fix ledger", "ledger", 10);
        let mut child = tagged("a1", "Edit", "ledger", 5);
        child.resume_payload.files.push("/elsewhere/x.rs".into());
        child.semantic =
            Some(SemanticIntentMetadata { workspace_root: Some("/home/u/ledger".into()), ..Default::default() });
        root.children.push(child);
        let intents = vec![root, tagged("b", "Fix site", "site", 20)];
        let request = ResumeSelectRequest {
            intent_id: Some(" a1 ".into()),
            restore_scope: Some(RestoreScope::SameProject),
            ..Default::default()
        };
        let r = select_resume_preview(&intents, &request, None, RerankOptions::default()).await.unwrap();
        let selected = r.selected.unwrap();
        assert_eq!(selected.resume_payload.files, vec!["/home/u/ledger/src/a.rs"]);
        assert!(selected.resume_payload.shell.cwd.is_some());

        let request = ResumeSelectRequest { project_tag: Some("SITE".into()), ..Default::default() };
        let r = select_resume_preview(&intents, &request, None, RerankOptions::default()).await.unwrap();
        assert_eq!(r.selected.unwrap().candidate.intent_id, "b");

        let request = ResumeSelectRequest { intent_id: Some("missing".into()), ..Default::default() };
        assert!(select_resume_preview(&intents, &request, None, RerankOptions::default()).await.is_none());
    }

    #[test]
    fn parents() {
        assert_eq!(pure_parent("/a/b.rs"), "/a");
        assert_eq!(pure_parent("/b.rs"), "/");
        assert_eq!(pure_parent("b.rs"), ".");
    }
}
