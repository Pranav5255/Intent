//! Deterministic intent engine for Intent 2.0.
//!
//! normalize → sessionize → cluster → correlate → (semantic refine, via trait) → cap
//! → label (via trait, template fallback) → enrich / resume → intent tree, plus the
//! read-side helpers: digest, context export, prediction, resume selection and
//! current intent. Pure functions and async orchestration only: no storage access,
//! no runtime dependency (timeouts use `futures-timer`).
//!
//! # Parity with 0.1
//!
//! `tests/golden.rs` runs [`pipeline::run`] in [`PipelineConfig::parity`] mode
//! against the 0.1 Python engine's output on four day fixtures and must match
//! boundaries, tree shape, labels, confidences, stats, tags, insights, prefixes and
//! resume payloads exactly (the `*.parity.json` fixtures are frozen).
//!
//! # Intentional divergences from 0.1
//!
//! * Command families come from `intent_types::COMMAND_FAMILIES` (adds cargo, go,
//!   pnpm, yarn, kubectl); the parity generator patches 0.1 with the same table.
//! * More code extensions count as `FileKind::Code` (`.rs`, `.tsx`, `.toml`, ...);
//!   URL domains also stop at `#`.
//! * `source_hash`: the payload is hashed once in canonical compact form (0.1
//!   serialized every normalized event, payload twice), and the cache identity also
//!   covers the correlation mode, pipeline version, session gap, cluster cap and
//!   evidence cap.
//! * Ids keep the uuid5 scheme, but hash the date, the scope and the earliest event
//!   id instead of the source hash and the session index (0.1). Later events and
//!   idle markers no longer change the ids of earlier intents. A one-task session
//!   takes the session id rather than its cluster id.
//! * New correlation step ([`correlate`]) merges similar adjacent clusters (default
//!   on; disabled in parity mode). The 4-cluster cap is re-applied after semantic
//!   refinement (0.1 applied none). Invalid refiner outcomes fall back with a warning.
//! * Intent evidence is capped at 40 deduplicated items (round-robin over events,
//!   values ≤240 chars); 0.1 copied every payload value into every intent and parent.
//! * Cluster/tree invariants produce `PipelineWarning`s instead of `assert`s.
//! * Labels: 1–7 words and 1–2 sentence summaries accepted and normalised; only
//!   unusable results are rejected (logged) and replaced by the template. Template
//!   summaries are prose built from the safe features instead of the numbered
//!   "1. Observed 2 browser events; ..." lines. Safe-feature lists are ordered by
//!   frequency; parent `command_families` are the children's dominant families
//!   (0.1: failed-command families only) and parents carry `project_key`.
//! * TODO markers come from the inserted text (0.1 always reported `TODO`).
//! * Digest: no hard-coded headline override; headline = primary label + project +
//!   other projects of the day; `top_intent_ids` ranked by duration.
//! * Context export words failures per family ("git command failed",
//!   "pytest tests failed 3 times (exit 1)") instead of "{family} test failed".
//! * Prediction caches its prefix index per (date, store revision).
//! * Current intent: label provider passed per call and part of the minute-keyed,
//!   bounded cache; provider failures fall back to the template.
//! * A lone `idle_start`/`idle_end` event yields no session (0.1 returned it).

pub mod cluster;
pub mod context;
pub mod correlate;
pub mod current;
pub mod digest;
pub mod enrich;
pub mod labeling;
pub mod normalize;
pub mod pipeline;
pub mod prediction;
pub mod resume;
pub mod resume_select;
pub mod sessionize;
pub mod util;

#[cfg(test)]
pub(crate) mod test_support;

pub use labeling::TemplateLabelProvider;
pub use pipeline::{PIPELINE_VERSION, PipelineConfig, PipelineDeps, PipelineInput, run};
