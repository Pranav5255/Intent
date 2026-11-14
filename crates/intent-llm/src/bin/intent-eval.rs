//! Correlation eval: runs labelled fixtures through the semantic refiner and reports
//! pairwise precision/recall/F1, ARI, latency and cache hits.
//!
//! ```text
//! intent-eval [--provider mock|local|openai|groq|gemini|bedrock] [--model M] [--base-url URL]
//!             [--api-key-env VAR] [--embed hashing|none|<model>] [--fixtures DIR] [--repeat N] [--json]
//! ```
//! `mock` (default) runs fully offline with a heuristic stand-in model.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use intent_llm::client::ChatClient;
use intent_llm::embed::{HashingEmbedder, OpenAiEmbedder, OpenAiEmbedderConfig};
use intent_llm::eval::{EvalReport, Fixture, run_fixture};
use intent_llm::harness::Harness;
use intent_llm::mock::HeuristicClient;
use intent_llm::semantic::{LlmSemanticRefiner, SemanticConfig};
use intent_llm::settings::{LlmSettings, build_chat_client};
use intent_llm::traits::Embedder;
use intent_types::LlmProvider;

struct Args {
    provider: String,
    model: Option<String>,
    base_url: Option<String>,
    api_key_env: Option<String>,
    embed: String,
    fixtures: Option<PathBuf>,
    repeat: usize,
    json: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        provider: "mock".into(),
        model: None,
        base_url: None,
        api_key_env: None,
        embed: "hashing".into(),
        fixtures: None,
        repeat: 2,
        json: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--provider" => args.provider = value()?,
            "--model" => args.model = Some(value()?),
            "--base-url" => args.base_url = Some(value()?),
            "--api-key-env" => args.api_key_env = Some(value()?),
            "--embed" => args.embed = value()?,
            "--fixtures" => args.fixtures = Some(PathBuf::from(value()?)),
            "--repeat" => args.repeat = value()?.parse().map_err(|_| "--repeat needs a number")?,
            "--json" => args.json = true,
            "-h" | "--help" => {
                println!(
                    "intent-eval [--provider mock|local|openai|groq|gemini|bedrock] [--model M] [--base-url URL]\n            [--api-key-env VAR] [--embed hashing|none|<model>] [--fixtures DIR] [--repeat N] [--json]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(args)
}

fn client(args: &Args) -> Result<Arc<dyn ChatClient>, String> {
    if args.provider == "mock" {
        return Ok(Arc::new(HeuristicClient::default()));
    }
    let provider: LlmProvider = serde_json::from_value(serde_json::json!(args.provider))
        .map_err(|_| format!("unknown provider {}", args.provider))?;
    let settings = LlmSettings {
        provider,
        model: args.model.clone(),
        base_url: args.base_url.clone(),
        api_key: args.api_key_env.as_ref().and_then(|v| std::env::var(v).ok()),
        ..Default::default()
    };
    build_chat_client(&settings)?.ok_or_else(|| "provider disabled".into())
}

fn embedder(args: &Args) -> Option<Arc<dyn Embedder>> {
    match args.embed.as_str() {
        "none" => None,
        "hashing" => Some(Arc::new(HashingEmbedder::default())),
        model => Some(Arc::new(OpenAiEmbedder::new(OpenAiEmbedderConfig {
            provider: "local".into(),
            base_url: args.base_url.clone().unwrap_or_else(|| intent_llm::settings::DEFAULT_LOCAL_BASE_URL.into()),
            api_key: None,
            model: model.into(),
            batch_size: 32,
            timeout: Duration::from_secs(30),
        }))),
    }
}

fn load_fixtures(dir: &Option<PathBuf>) -> Result<Vec<Fixture>, String> {
    let Some(dir) = dir else { return Ok(Fixture::builtin()) };
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    paths.iter().map(|p| Fixture::parse(&std::fs::read_to_string(p).map_err(|e| e.to_string())?)).collect()
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("intent-eval: {e}");
        std::process::exit(2);
    }
}

async fn run() -> Result<(), String> {
    let args = parse_args()?;
    let client = client(&args)?;
    let identity = client.identity();
    let harness = Arc::new(Harness::for_client(client));
    let mut config = SemanticConfig::for_identity(&identity);
    config.content_consent = true;
    let refiner = LlmSemanticRefiner::new(Arc::clone(&harness), embedder(&args), config);
    let fixtures = load_fixtures(&args.fixtures)?;

    let mut reports: Vec<(usize, EvalReport)> = Vec::new();
    for round in 0..args.repeat.max(1) {
        for f in &fixtures {
            reports.push((round, run_fixture(&refiner, f).await));
        }
    }
    let stats = harness.stats();
    if args.json {
        let out = serde_json::json!({
            "provider": identity.provider, "model": identity.model, "embedder": args.embed,
            "reports": reports.iter().map(|(r, rep)| serde_json::json!({"round": r, "report": rep})).collect::<Vec<_>>(),
            "harness": {"calls": stats.calls, "provider_attempts": stats.provider_attempts, "repairs": stats.repairs,
                        "failures": stats.failures, "cache_hits": stats.cache_hits, "cache_misses": stats.cache_misses},
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
        return Ok(());
    }
    println!("provider={} model={} embedder={}", identity.provider, identity.model, args.embed);
    println!(
        "{:<5} {:<24} {:>4} {:>4} {:>6} {:>6} {:>6} {:>6} {:>9} {:>8}  fallback",
        "round", "fixture", "n", "k", "P", "R", "F1", "ARI", "base F1", "ms"
    );
    for (round, r) in &reports {
        println!(
            "{:<5} {:<24} {:>4} {:>4} {:>6.3} {:>6.3} {:>6.3} {:>6.3} {:>9.3} {:>8}  {}",
            round,
            r.fixture,
            r.events,
            r.clusters,
            r.scores.precision,
            r.scores.recall,
            r.scores.f1,
            r.scores.ari,
            r.baseline.f1,
            r.latency_ms,
            r.fallback.as_deref().unwrap_or("-")
        );
    }
    let first: Vec<&EvalReport> = reports.iter().filter(|(r, _)| *r == 0).map(|(_, r)| r).collect();
    let mean = |f: &dyn Fn(&EvalReport) -> f64| first.iter().map(|r| f(r)).sum::<f64>() / first.len().max(1) as f64;
    println!(
        "mean (round 0): P={:.3} R={:.3} F1={:.3} ARI={:.3} baseline F1={:.3}",
        mean(&|r| r.scores.precision),
        mean(&|r| r.scores.recall),
        mean(&|r| r.scores.f1),
        mean(&|r| r.scores.ari),
        mean(&|r| r.baseline.f1)
    );
    println!(
        "harness: calls={} provider_attempts={} repairs={} failures={} cache_hits={} cache_misses={}",
        stats.calls, stats.provider_attempts, stats.repairs, stats.failures, stats.cache_hits, stats.cache_misses
    );
    Ok(())
}
