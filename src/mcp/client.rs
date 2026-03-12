use crate::error::McpError;

pub struct PostbloxClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl PostbloxClient {
    pub fn new(base_url: String, api_key: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build http client");
        Self {
            http,
            base_url,
            api_key,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}/api/v1{}", self.base_url.trim_end_matches('/'), path)
    }

    pub async fn get(&self, path: &str) -> Result<String, McpError> {
        let resp = self
            .http
            .get(self.url(path))
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        Self::handle_response(resp).await
    }

    pub async fn post(&self, path: &str, body: serde_json::Value) -> Result<String, McpError> {
        let resp = self
            .http
            .post(self.url(path))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
        Self::handle_response(resp).await
    }

    pub async fn put(&self, path: &str, body: serde_json::Value) -> Result<String, McpError> {
        let resp = self
            .http
            .put(self.url(path))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
        Self::handle_response(resp).await
    }

    pub async fn delete(&self, path: &str) -> Result<String, McpError> {
        let resp = self
            .http
            .delete(self.url(path))
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        Self::handle_response(resp).await
    }

    async fn handle_response(resp: reqwest::Response) -> Result<String, McpError> {
        let status = resp.status();
        let body = resp.text().await?;
        if status.is_success() {
            Ok(body)
        } else {
            Err(McpError::Api(format!("{status}: {body}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_building_basic_path() {
        let client = PostbloxClient::new("http://localhost:3000".into(), "key".into());
        assert_eq!(
            client.url("/inboxes"),
            "http://localhost:3000/api/v1/inboxes"
        );
    }

    #[test]
    fn test_url_building_trailing_slash_stripped() {
        let client = PostbloxClient::new("http://localhost:3000/".into(), "key".into());
        assert_eq!(
            client.url("/inboxes"),
            "http://localhost:3000/api/v1/inboxes"
        );
    }

    #[test]
    fn test_url_building_nested_path() {
        let client = PostbloxClient::new("http://localhost:3000".into(), "key".into());
        let id = "550e8400-e29b-41d4-a716-446655440000";
        assert_eq!(
            client.url(&format!("/inboxes/{id}/messages")),
            format!("http://localhost:3000/api/v1/inboxes/{id}/messages")
        );
    }

    #[test]
    fn test_url_building_with_custom_port() {
        let client = PostbloxClient::new("http://mail.example.com:8080".into(), "key".into());
        assert_eq!(
            client.url("/search"),
            "http://mail.example.com:8080/api/v1/search"
        );
    }
}
