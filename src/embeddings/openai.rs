use std::future::Future;
use std::pin::Pin;

use super::{EmbeddingError, EmbeddingProvider};

pub struct OpenAiProvider {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: Option<String>,
    dimensions: usize,
}

impl OpenAiProvider {
    pub fn new(
        base_url: &str,
        model: &str,
        api_key: Option<String>,
        dimensions: usize,
    ) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            api_key,
            dimensions,
        })
    }
}

#[derive(serde::Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

#[derive(serde::Deserialize)]
struct EmbedResponse {
    data: Vec<EmbeddingData>,
}

impl EmbeddingProvider for OpenAiProvider {
    fn embed(
        &self,
        text: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<f32>, EmbeddingError>> + Send + '_>> {
        let text = text.to_string();
        Box::pin(async move {
            let mut req = self
                .client
                .post(format!("{}/v1/embeddings", self.base_url))
                .json(&serde_json::json!({
                    "model": self.model,
                    "input": text,
                }));
            if let Some(ref key) = self.api_key {
                req = req.bearer_auth(key);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| EmbeddingError::Request(e.to_string()))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp
                    .text()
                    .await
                    .unwrap_or_else(|e| format!("(body read failed: {e})"));
                return Err(EmbeddingError::Request(format!("{status}: {body}")));
            }

            let body: EmbedResponse = resp
                .json()
                .await
                .map_err(|e| EmbeddingError::InvalidResponse(e.to_string()))?;

            let embedding = body
                .data
                .into_iter()
                .next()
                .map(|d| d.embedding)
                .ok_or_else(|| EmbeddingError::InvalidResponse("empty data array".into()))?;

            if embedding.len() != self.dimensions {
                return Err(EmbeddingError::DimensionMismatch {
                    expected: self.dimensions,
                    actual: embedding.len(),
                });
            }

            Ok(embedding)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_new_trims_trailing_slash() {
        let p =
            OpenAiProvider::new("http://localhost:11434/", "nomic-embed-text", None, 768).unwrap();
        assert_eq!(p.base_url, "http://localhost:11434");
    }

    #[test]
    fn test_provider_new_preserves_clean_url() {
        let p = OpenAiProvider::new("http://localhost:11434", "test-model", None, 384).unwrap();
        assert_eq!(p.base_url, "http://localhost:11434");
        assert_eq!(p.model, "test-model");
        assert_eq!(p.dimensions, 384);
    }

    #[test]
    fn test_embed_response_deserialize_valid() {
        let json = r#"{"data": [{"embedding": [0.1, -0.2, 0.3], "index": 0}]}"#;
        let resp: EmbedResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.data.len(), 1);
        assert_eq!(resp.data[0].embedding.len(), 3);
    }

    #[test]
    fn test_embed_response_deserialize_empty_data() {
        let json = r#"{"data": []}"#;
        let resp: EmbedResponse = serde_json::from_str(json).unwrap();
        assert!(resp.data.is_empty());
    }

    #[test]
    fn test_embed_response_deserialize_multiple_embeddings() {
        let json = r#"{"data": [{"embedding": [0.1, 0.2], "index": 0}, {"embedding": [0.3, 0.4], "index": 1}]}"#;
        let resp: EmbedResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.data.len(), 2);
    }

    #[test]
    fn test_embedding_error_display() {
        let err = EmbeddingError::DimensionMismatch {
            expected: 768,
            actual: 384,
        };
        assert!(err.to_string().contains("768"));
        assert!(err.to_string().contains("384"));
    }
}
