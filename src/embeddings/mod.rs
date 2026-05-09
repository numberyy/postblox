//! Embedding provider abstraction for semantic search.
//!
//! Defines [`EmbeddingProvider`], a trait every backend implements. A
//! single concrete impl ([`openai::OpenAiProvider`]) ships today; the
//! trait is kept anyway because it's the seam tests mock at — the
//! AGENTS.md rule "Mock at trait boundaries, not at depth" applies
//! here, and search code should never depend on a concrete provider
//! type.
//!
//! Like [`crate::mail`], this module is framework-free at the source-
//! tree level: the trait itself pulls in no async runtime. Concrete
//! impls (e.g. [`openai`]) are free to use `reqwest` / `tokio`
//! internally, but callers stay generic over `dyn EmbeddingProvider`.
//!
//! Errors funnel through [`EmbeddingError`], including a
//! [`EmbeddingError::DimensionMismatch`] guard for the dimension the
//! provider was configured with.

pub mod openai;

use std::future::Future;
use std::pin::Pin;

#[derive(Debug, thiserror::Error)]
pub enum EmbeddingError {
    #[error("embedding request failed: {0}")]
    Request(String),
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    #[error("dimension mismatch: expected {expected}, got {actual}")]
    DimensionMismatch { expected: usize, actual: usize },
}

pub trait EmbeddingProvider: Send + Sync {
    fn embed(
        &self,
        text: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<f32>, EmbeddingError>> + Send + '_>>;
}
