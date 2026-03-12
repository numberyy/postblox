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

    #[allow(dead_code)] // part of provider interface, used for validation
    fn dimensions(&self) -> usize;
}
