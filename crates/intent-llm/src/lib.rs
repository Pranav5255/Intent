//! Inference harness: providers, structured output, embeddings, semantic refinement.
//!
//! `intent-engine` depends only on the traits in [`traits`]; everything else in this
//! crate is an implementation detail behind them.

pub mod traits;

pub use traits::*;
