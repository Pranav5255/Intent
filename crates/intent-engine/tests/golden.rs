//! Golden tests over day fixtures.
//!
//! * `*.parity.json` — structural output of the 1.x Python engine (template labels,
//!   semantic off), produced by `fixtures/generate_parity.py`. The Rust pipeline in
//!   parity mode (no correlation step) must match it exactly.
//! * `*.rust.json` — full 2.0 output with default settings (correlation on), plus the
//!   digest and context export. Regenerate with `UPDATE_GOLDEN=1 cargo test -p intent-engine`.

use std::path::PathBuf;

use intent_engine::context::build_intent_context;
use intent_engine::digest::build_digest;
use intent_engine::pipeline::{self, PipelineConfig, PipelineDeps, PipelineInput};
use intent_types::{DayExport, Intent};
use serde_json::{Value, json};

const FIXTURES: &[&str] = &["sample-day", "coding-research-day", "debugging-day", "interrupted-day"];

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn load(name: &str) -> PipelineInput {
    let text = std::fs::read_to_string(fixture_dir().join(format!("{name}.json"))).expect("fixture");
    let export: DayExport = serde_json::from_str(&text).expect("valid day export");
    export.into()
}

fn read_json(path: &PathBuf) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
        .expect("json")
}

/// Same projection as `generate_parity.py::project`.
fn project(intent: &Intent) -> Value {
    let v = serde_json::to_value(intent).expect("intent json");
    json!({
        "label": v["label"],
        "confidence": v["confidence"],
        "depth": v["depth"],
        "start_ts": v["start_ts"],
        "end_ts": v["end_ts"],
        "event_count": v["stats"]["event_count"],
        "duration_seconds": v["stats"]["duration_seconds"],
        "sources": v["stats"]["sources"],
        "tags": v["tags"],
        "prefix": v["prefix"],
        "insights": v["insights"],
        "todos": intent.todos.iter().map(|t| json!({"path": t.path, "observed_ts": t.observed_ts})).collect::<Vec<_>>(),
        "resume_payload": {
            "files": v["resume_payload"]["files"],
            "urls": v["resume_payload"]["urls"],
            "shell": v["resume_payload"]["shell"],
        },
        "children": intent.children.iter().map(project).collect::<Vec<_>>(),
    })
}

fn outline(intents: &[Intent]) -> String {
    let mut out = String::new();
    for root in intents {
        out.push_str(&format!("{} ({} events) {:?}\n", root.label, root.stats.event_count, root.tags));
        for child in &root.children {
            out.push_str(&format!("    {} ({} events)\n", child.label, child.stats.event_count));
        }
    }
    out
}

#[tokio::test]
async fn parity_with_python_engine() {
    let deps = PipelineDeps { config: PipelineConfig::parity(), ..PipelineDeps::template_only() };
    for name in FIXTURES {
        let result = pipeline::run(load(name), &deps).await;
        assert!(result.warnings.is_empty(), "{name}: {:?}", result.warnings);
        let actual = json!({
            "date": load(name).date,
            "intents": result.intents.iter().map(project).collect::<Vec<_>>(),
        });
        let expected = read_json(&fixture_dir().join(format!("{name}.parity.json")));
        if actual != expected {
            panic!(
                "{name}: parity mismatch\n--- rust ---\n{}\n--- python ---\n{}",
                serde_json::to_string_pretty(&actual).unwrap_or_default(),
                serde_json::to_string_pretty(&expected).unwrap_or_default()
            );
        }
    }
}

#[tokio::test]
async fn default_pipeline_snapshots() {
    let deps = PipelineDeps::template_only();
    let update = std::env::var_os("UPDATE_GOLDEN").is_some();
    for name in FIXTURES {
        let input = load(name);
        let date = input.date.clone();
        let result = pipeline::run(input, &deps).await;
        let digest = build_digest(&result.intents, &date);
        let actual = json!({
            "outline": outline(&result.intents).lines().collect::<Vec<_>>(),
            "digest": digest,
            "contexts": result.intents.iter().map(build_intent_context).collect::<Vec<_>>(),
            "result": result,
        });
        let path = fixture_dir().join(format!("{name}.rust.json"));
        if update {
            std::fs::write(&path, serde_json::to_string_pretty(&actual).unwrap_or_default() + "\n").expect("write");
            continue;
        }
        let expected = read_json(&path);
        assert_eq!(actual["outline"], expected["outline"], "{name}: outline changed");
        assert_eq!(actual, expected, "{name}: snapshot changed (UPDATE_GOLDEN=1 to accept)");
    }
}

#[tokio::test]
async fn pipeline_is_deterministic_and_order_independent() {
    let deps = PipelineDeps::template_only();
    for name in FIXTURES {
        let a = pipeline::run(load(name), &deps).await;
        let mut shuffled = load(name);
        shuffled.events.reverse();
        let b = pipeline::run(shuffled, &deps).await;
        // Ordinals (original positions) are part of the hash, so ids differ, but the
        // tree content does not.
        let strip = |r: &intent_types::PipelineResult| r.intents.iter().map(project).collect::<Vec<_>>();
        assert_eq!(strip(&a), strip(&b), "{name}");
        assert_eq!(a, pipeline::run(load(name), &deps).await, "{name}: not deterministic");
    }
}

#[tokio::test]
async fn invariants_hold_on_every_fixture() {
    let deps = PipelineDeps::template_only();
    for name in FIXTURES {
        let result = pipeline::run(load(name), &deps).await;
        for root in &result.intents {
            assert!(root.children.len() <= 4, "{name}: cap");
            assert_eq!(root.depth, 0);
            assert!(root.evidence.len() <= pipeline::MAX_INTENT_EVIDENCE);
            assert!(root.resume_payload.files.len() <= 5 && root.resume_payload.urls.len() <= 8);
            for child in &root.children {
                assert_eq!(child.parent_id.as_deref(), Some(root.id.as_str()));
                assert!(child.evidence.len() <= pipeline::MAX_INTENT_EVIDENCE);
                assert!((1..=7).contains(&child.label.split_whitespace().count()));
            }
            assert!(intent_engine::enrich::validate_intent_tree(root).is_empty());
        }
    }
}
