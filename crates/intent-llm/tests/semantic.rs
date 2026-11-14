//! Semantic refinement end-to-end with offline clients.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use intent_llm::client::{ChatClient, ChatMessage, ClientIdentity, JsonRequest, ToolSpec, ToolTurn};
use intent_llm::embed::HashingEmbedder;
use intent_llm::eval::{Fixture, assignment, run_fixture, score};
use intent_llm::harness::Harness;
use intent_llm::mock::{HeuristicClient, ScriptedClient};
use intent_llm::semantic::{LlmSemanticRefiner, SemanticConfig};
use intent_llm::traits::{Embedder, LlmError, SemanticFallback, SemanticRefiner};
use serde_json::{Value, json};

fn fixture(name: &str) -> Fixture {
    Fixture::builtin().into_iter().find(|f| f.name == name).unwrap()
}

fn refiner(client: Arc<dyn ChatClient>, embedder: Option<Arc<dyn Embedder>>) -> LlmSemanticRefiner {
    let identity = client.identity();
    let harness = Arc::new(Harness::for_client(client));
    let mut config = SemanticConfig::for_identity(&identity);
    config.content_consent = true;
    LlmSemanticRefiner::new(harness, embedder, config)
}

fn ids(session: &[intent_types::NormalizedEvent], cluster: &[usize]) -> Vec<String> {
    cluster.iter().map(|&i| session[i].id.clone()).collect()
}

#[tokio::test]
async fn heuristic_model_recovers_fixture_tasks() {
    let r = refiner(Arc::new(HeuristicClient::default()), Some(Arc::new(HashingEmbedder::default())));
    for f in Fixture::builtin() {
        let report = run_fixture(&r, &f).await;
        assert!(report.fallback.is_none(), "{}: {:?}", f.name, report.fallback);
        assert!(report.scores.precision >= 0.99, "{}: precision {:?}", f.name, report.scores);
        assert!(
            report.scores.f1 >= report.baseline.f1,
            "{}: {:?} vs baseline {:?}",
            f.name,
            report.scores,
            report.baseline
        );
    }
}

#[tokio::test]
async fn excluded_messaging_stays_singleton_and_workspaces_never_merge() {
    let r = refiner(Arc::new(HeuristicClient::default()), Some(Arc::new(HashingEmbedder::default())));
    let f = fixture("research_writing");
    let (session, _) = f.session();
    let out = r.refine(&session, &Fixture::deterministic(&session)).await.unwrap();
    assert_eq!(out.clusters.len(), out.semantics.len());
    let slack = out.clusters.iter().find(|c| ids(&session, c).contains(&"s1".to_string())).unwrap();
    assert_eq!(slack.len(), 1);
    let all: usize = out.clusters.iter().map(Vec::len).sum();
    assert_eq!(all, session.len(), "no event dropped");

    let f = fixture("interleaved_projects");
    let (session, _) = f.session();
    let out = r.refine(&session, &Fixture::deterministic(&session)).await.unwrap();
    for c in &out.clusters {
        let projects: std::collections::BTreeSet<&String> =
            c.iter().flat_map(|&i| session[i].entities.project_paths.iter()).collect();
        assert!(projects.len() <= 1, "cluster mixes workspaces: {:?}", ids(&session, c));
    }
}

#[tokio::test]
async fn semantics_carry_roles_and_topic() {
    let r = refiner(Arc::new(HeuristicClient::default()), None);
    let f = fixture("ops_incident");
    let (session, _) = f.session();
    let out = r.refine(&session, &[]).await.unwrap();
    let first = &out.semantics[0];
    assert_eq!(first.topic.as_deref(), Some("/home/u/platform"));
    assert!(first.roles.get("task").copied().unwrap_or(0) > 0);
    assert!(first.confidence.unwrap() > 0.5);
}

fn proposals(items: &[(&str, &str, f64, &[&str])]) -> String {
    json!({"proposals": items.iter().map(|(id, role, c, links)| json!({
        "event_id": id, "role": role, "confidence": c, "topic": "t", "linked_event_ids": links
    })).collect::<Vec<_>>()})
    .to_string()
}

#[tokio::test]
async fn invalid_proposals_are_repaired_once_then_fall_back() {
    let f = fixture("ops_incident"); // packets p0..p2
    let (session, _) = f.session();

    let scripted = Arc::new(ScriptedClient::new(true));
    scripted.push_json(Ok(proposals(&[("p0", "task", 0.9, &[]), ("p1", "unrelated", 0.9, &[])]))); // p2 missing
    scripted.push_json(Ok(proposals(&[
        ("p0", "task", 0.9, &["p2"]),
        ("p1", "unrelated", 0.9, &[]),
        ("p2", "task", 0.9, &["p0"]),
    ])));
    let r = refiner(scripted.clone(), None);
    let out = r.refine(&session, &[]).await.unwrap();
    assert_eq!(scripted.json_calls(), 2);
    let repair = scripted.requests.lock().unwrap()[1].repair.clone().unwrap();
    assert!(repair.error.contains("p2"), "{}", repair.error);
    assert_eq!(out.clusters.len(), 2, "{:?}", out.clusters);

    let scripted = Arc::new(ScriptedClient::new(true));
    scripted.push_json(Ok(proposals(&[("p0", "task", 0.9, &["p9"])])));
    scripted.push_json(Ok("still not right".into()));
    let r = refiner(scripted.clone(), None);
    assert_eq!(r.refine(&session, &[]).await.unwrap_err(), SemanticFallback::InvalidResponse);
}

#[tokio::test]
async fn provider_errors_map_to_fallbacks() {
    let f = fixture("ops_incident");
    let (session, _) = f.session();
    let scripted = Arc::new(ScriptedClient::new(true));
    scripted.push_json(Err(LlmError::http(401, "nope", None, None)));
    let r = refiner(scripted, None);
    assert_eq!(r.refine(&session, &[]).await.unwrap_err(), SemanticFallback::ProviderUnavailable);
}

#[tokio::test]
async fn borderline_links_need_embedding_support() {
    let f = fixture("ops_incident");
    let (session, labels) = f.session();
    // p0 (kubectl+grafana) and p2 (deploy.yaml + git) linked at a borderline 0.6.
    let reply = proposals(&[("p0", "task", 0.6, &["p2"]), ("p1", "unrelated", 0.9, &[]), ("p2", "task", 0.6, &["p0"])]);

    let without = Arc::new(ScriptedClient::new(true));
    without.push_json(Ok(reply.clone()));
    let out = refiner(without, None).refine(&session, &[]).await.unwrap();
    assert_eq!(out.clusters.len(), 3, "no embedding support: borderline link rejected");

    // A permissive embedder makes every pair similar.
    struct Same;
    #[async_trait]
    impl Embedder for Same {
        fn cache_identity(&self) -> String {
            "same".into()
        }
        fn dims(&self) -> usize {
            2
        }
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LlmError> {
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }
    }
    let with = Arc::new(ScriptedClient::new(true));
    with.push_json(Ok(reply));
    let out = refiner(with.clone(), Some(Arc::new(Same))).refine(&session, &[]).await.unwrap();
    assert_eq!(out.clusters.len(), 2);
    let s = score(&assignment(session.len(), &out.clusters), &labels);
    assert!((s.f1 - 1.0).abs() < 1e-9, "{s:?}");
    // The similarity hint reached the prompt.
    let user = with.requests.lock().unwrap()[0].user.clone();
    assert!(user.contains("\"sim_prev\":1.0"), "{user}");
}

/// Sleeps per request and records the peak number of concurrent requests.
struct SlowCloud {
    inner: HeuristicClient,
    in_flight: AtomicUsize,
    peak: AtomicUsize,
}

#[async_trait]
impl ChatClient for SlowCloud {
    fn identity(&self) -> ClientIdentity {
        ClientIdentity {
            provider: "mock-cloud".into(),
            model: "slow".into(),
            endpoint: "memory".into(),
            is_local: false,
        }
    }
    async fn respond_json(&self, request: &JsonRequest) -> Result<String, LlmError> {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(150)).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.inner.respond_json(request).await
    }
    async fn respond_with_tools(&self, _: &str, _: &[ChatMessage], _: &[ToolSpec]) -> Result<ToolTurn, LlmError> {
        unreachable!()
    }
}

fn long_session(n: usize) -> Vec<intent_types::NormalizedEvent> {
    let mut f = fixture("auth_bugfix_with_docs");
    let template = f.events.clone();
    f.events.clear();
    for i in 0..n {
        let mut e = template[i % template.len()].clone();
        e.id = format!("e{i}");
        e.t = i as i64 * 400; // every event its own packet
        f.events.push(e);
    }
    f.session().0
}

#[tokio::test]
async fn batches_are_bounded_overlapping_and_sent_concurrently() {
    let client = Arc::new(SlowCloud {
        inner: HeuristicClient::default(),
        in_flight: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
    });
    let harness = Arc::new(Harness::for_client(client.clone()));
    let mut config = SemanticConfig::for_identity(&client.identity());
    config.max_prompt_chars = 1200;
    let r = LlmSemanticRefiner::new(harness.clone(), None, config);
    let session = long_session(40);
    let out = r.refine(&session, &[]).await.unwrap();
    assert_eq!(out.clusters.iter().map(Vec::len).sum::<usize>(), 40);
    let calls = harness.stats().provider_attempts;
    assert!(calls >= 4, "expected several batches, got {calls}");
    assert!(client.peak.load(Ordering::SeqCst) > 1, "batches were not concurrent");
    assert!(client.peak.load(Ordering::SeqCst) <= 4, "cloud limiter is 4");
}

#[test]
fn batch_planner_is_linear_and_respects_limit() {
    let client: Arc<dyn ChatClient> = Arc::new(HeuristicClient::default());
    let harness = Arc::new(Harness::for_client(client.clone()));
    let mut config = SemanticConfig::for_identity(&client.identity());
    config.max_prompt_chars = 1500;
    let r = LlmSemanticRefiner::new(harness, None, config);
    let session = long_session(300);
    let started = std::time::Instant::now();
    let requests = r.request_packets(&session, &[]);
    assert_eq!(requests.len(), 300);
    let batches = r.batches(&requests).unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    for (i, b) in batches.iter().enumerate() {
        let body: Vec<&str> = b.iter().map(|&k| requests[k].json.as_str()).collect();
        let prompt = format!("{{\"packets\":[{}]}}", body.join(","));
        assert!(prompt.len() <= 1500, "batch {i} is {} chars", prompt.len());
        let _: Value = serde_json::from_str(&prompt).unwrap();
        if i > 0 {
            assert_eq!(b[0], *batches[i - 1].last().unwrap(), "one-packet overlap between batches");
        }
    }
    let covered: std::collections::BTreeSet<usize> = batches.iter().flatten().copied().collect();
    assert_eq!(covered.len(), 300);
}

#[tokio::test]
async fn full_capture_uses_the_lossless_codec() {
    let f = fixture("ops_incident");
    let (session, _) = f.session();
    let scripted = Arc::new(ScriptedClient::new(false));
    scripted.push_json(Ok(proposals(&[
        ("p0", "task", 0.9, &["p2"]),
        ("p1", "unrelated", 0.9, &[]),
        ("p2", "task", 0.9, &["p0"]),
    ])));
    let identity = scripted.identity();
    let mut config = SemanticConfig::for_identity(&identity);
    config.content_consent = true;
    config.full_capture_consent = true;
    let r = LlmSemanticRefiner::new(Arc::new(Harness::for_client(scripted.clone())), None, config);
    assert!(r.cache_identity().contains(":full:"));
    let out = r.refine(&session, &[]).await.unwrap();
    assert_eq!(out.clusters.len(), 2);

    let req = scripted.requests.lock().unwrap()[0].clone();
    assert!(req.system.starts_with("Full-capture"));
    let prompt: Value = serde_json::from_str(&req.user).unwrap();
    assert_eq!(prompt["output_packet_ids"], json!(["p0", "p1", "p2"]));
    assert_eq!(prompt["events"].as_array().unwrap().len(), session.len());
    assert!(req.user.contains("kubectl get pods -n api"), "full capture keeps raw payload values");
    // Strings that repeat often enough are moved into the dictionary and referenced as @N.
    let dictionary = prompt["dictionary"].as_array().unwrap();
    if !dictionary.is_empty() {
        assert!(req.user.contains("\"@0\""), "dictionary entries are referenced");
    }
    assert!(prompt["key_legend"].is_object());
}
