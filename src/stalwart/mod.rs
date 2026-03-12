use std::time::Duration;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum StalwartError {
    #[error("stalwart request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("stalwart returned {status}: {body}")]
    Api { status: u16, body: String },
}

#[derive(Clone)]
pub struct StalwartClient {
    http: reqwest::Client,
    base_url: String,
    admin_token: String,
}

impl StalwartClient {
    pub fn new(base_url: &str, admin_token: &str) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("failed to build http client");

        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            admin_token: admin_token.to_string(),
        }
    }

    async fn check_response(resp: reqwest::Response) -> Result<reqwest::Response, StalwartError> {
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(StalwartError::Api { status, body });
        }
        Ok(resp)
    }

    pub async fn create_account(&self, email: &str, password: &str) -> Result<(), StalwartError> {
        let resp = self
            .http
            .post(format!("{}/api/principal", self.base_url))
            .bearer_auth(&self.admin_token)
            .json(&serde_json::json!({
                "type": "individual",
                "name": email,
                "secrets": [password],
                "emails": [email],
            }))
            .send()
            .await?;

        Self::check_response(resp).await?;
        Ok(())
    }

    pub async fn delete_account(&self, email: &str) -> Result<(), StalwartError> {
        let resp = self
            .http
            .delete(format!("{}/api/principal/{email}", self.base_url))
            .bearer_auth(&self.admin_token)
            .send()
            .await?;

        Self::check_response(resp).await?;
        Ok(())
    }

    pub async fn create_domain(&self, name: &str) -> Result<String, StalwartError> {
        let resp = self
            .http
            .post(format!("{}/api/principal", self.base_url))
            .bearer_auth(&self.admin_token)
            .json(&serde_json::json!({
                "type": "domain",
                "name": name,
            }))
            .send()
            .await?;

        let resp = Self::check_response(resp).await?;
        let body: serde_json::Value = resp.json().await?;
        let principal_id = body["data"]["id"].as_str().unwrap_or(name).to_string();
        Ok(principal_id)
    }

    pub async fn delete_domain(&self, principal_id: &str) -> Result<(), StalwartError> {
        let resp = self
            .http
            .delete(format!("{}/api/principal/{principal_id}", self.base_url))
            .bearer_auth(&self.admin_token)
            .send()
            .await?;

        Self::check_response(resp).await?;
        Ok(())
    }

    pub async fn get_dns_records(&self, domain: &str) -> Result<serde_json::Value, StalwartError> {
        let resp = self
            .http
            .get(format!("{}/api/dns/records/{domain}", self.base_url))
            .bearer_auth(&self.admin_token)
            .send()
            .await?;

        let resp = Self::check_response(resp).await?;
        let body: serde_json::Value = resp.json().await?;
        Ok(body)
    }

    pub async fn submit_message(
        &self,
        from: &str,
        to: &[&str],
        raw_mime: Vec<u8>,
    ) -> Result<(), StalwartError> {
        let mut req = self
            .http
            .post(format!("{}/api/queue/messages", self.base_url))
            .bearer_auth(&self.admin_token)
            .header("content-type", "message/rfc822")
            .query(&[("from", from)]);

        for addr in to {
            req = req.query(&[("to", addr)]);
        }

        let resp = req.body(raw_mime).send().await?;

        Self::check_response(resp).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stalwart_client_trims_trailing_slash() {
        let client = StalwartClient::new("http://localhost:8080/", "token");
        assert_eq!(client.base_url, "http://localhost:8080");
    }

    #[test]
    fn test_stalwart_client_no_trailing_slash() {
        let client = StalwartClient::new("http://localhost:8080", "token");
        assert_eq!(client.base_url, "http://localhost:8080");
    }

    #[test]
    fn test_stalwart_error_display_api() {
        let err = StalwartError::Api {
            status: 404,
            body: "not found".into(),
        };
        assert!(err.to_string().contains("404"));
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn test_stalwart_error_display_api_500() {
        let err = StalwartError::Api {
            status: 500,
            body: "internal error".into(),
        };
        assert!(err.to_string().contains("500"));
        assert!(err.to_string().contains("internal error"));
    }

    #[test]
    fn test_stalwart_create_domain_url() {
        let client = StalwartClient::new("http://localhost:8080", "token");
        let url = format!("{}/api/principal", client.base_url);
        assert_eq!(url, "http://localhost:8080/api/principal");
    }

    #[test]
    fn test_stalwart_delete_domain_url() {
        let client = StalwartClient::new("http://localhost:8080/", "token");
        let url = format!("{}/api/principal/{}", client.base_url, "domain-123");
        assert_eq!(url, "http://localhost:8080/api/principal/domain-123");
    }

    #[test]
    fn test_stalwart_dns_records_url() {
        let client = StalwartClient::new("http://localhost:8080", "token");
        let url = format!("{}/api/dns/records/{}", client.base_url, "example.com");
        assert_eq!(url, "http://localhost:8080/api/dns/records/example.com");
    }

    #[tokio::test]
    #[ignore] // requires running Stalwart server
    async fn test_stalwart_create_and_delete_account() {
        let client = StalwartClient::new("http://localhost:8080", "admin-token");
        let email = format!("test-{}@postblox.local", uuid::Uuid::new_v4());
        client.create_account(&email, "password123").await.unwrap();
        client.delete_account(&email).await.unwrap();
    }
}
