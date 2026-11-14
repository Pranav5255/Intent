//! Error type shared by every store operation.

use thiserror::Error;

/// Everything that can go wrong inside `intent-store`.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Caller passed something the store refuses to act on (bad date, bad limit,
    /// wrong embedding dimension, ...). HTTP layers should map this to 400.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// The embeddings table has not been configured (no dimension known yet).
    #[error("embeddings are not configured")]
    EmbeddingsDisabled,
}

pub type Result<T, E = StoreError> = std::result::Result<T, E>;

pub(crate) fn invalid(msg: impl Into<String>) -> StoreError {
    StoreError::InvalidArgument(msg.into())
}
