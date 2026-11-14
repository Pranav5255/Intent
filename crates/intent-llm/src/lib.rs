//! Inference harness: providers, structured output, embeddings, semantic refinement.
//!
//! `intent-engine` depends only on the traits in [`traits`]; everything else in this
//! crate is an implementation detail behind them. The daemon builds everything from
//! [`settings::LlmSettings`] via [`settings::build_providers`].
//!
//! Local-first: the default provider is an OpenAI-compatible server on loopback
//! (Ollama, llama.cpp `llama-server`, LM Studio). Cloud providers are optional.

pub mod client;
pub mod copilot;
pub mod embed;
pub mod eval;
pub mod harness;
pub mod label;
pub mod mock;
pub mod rewrite;
pub mod semantic;
pub mod settings;
pub mod traits;
pub mod util;

pub use client::{ChatClient, ClientIdentity};
pub use copilot::{Copilot, CopilotBackend, CopilotConfig, IntentStatsReport, ToolMode};
pub use embed::{HashingEmbedder, OpenAiEmbedder};
pub use harness::{CachePersistence, Harness, HarnessConfig, HarnessError};
pub use label::LlmLabelProvider;
pub use semantic::{LlmSemanticRefiner, SemanticConfig};
pub use settings::{LlmSettings, Providers, build_providers, probe_local_server};
pub use traits::*;
