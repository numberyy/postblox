use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

// TUI-local types — separate binary, no imports from crate::models

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inbox {
    pub id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub inbox_type: String,
    pub active: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: Uuid,
    pub inbox_id: Uuid,
    pub thread_id: Option<Uuid>,
    pub from_addr: String,
    pub to_addrs: serde_json::Value,
    pub subject: Option<String>,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
    pub direction: String,
    pub created_at: DateTime<Utc>,
    pub slop_score: Option<f32>,
    pub category: Option<String>,
    pub triage_status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub id: Uuid,
    pub inbox_id: Uuid,
    pub subject: Option<String>,
    pub message_count: i32,
    pub last_message_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approval {
    pub id: Uuid,
    pub inbox_id: Uuid,
    pub message_id: Uuid,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub subject: Option<String>,
    pub from_addr: Option<String>,
    pub inbox_email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Briefing {
    pub period: String,
    pub since: DateTime<Utc>,
    pub total_received: i64,
    pub total_sent: i64,
    pub by_inbox: Vec<InboxStats>,
    pub top_senders: Vec<SenderCount>,
    pub top_subjects: Vec<SubjectCount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxStats {
    pub inbox_id: Uuid,
    pub inbox_email: String,
    pub received: i64,
    pub sent: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SenderCount {
    pub address: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubjectCount {
    pub subject: String,
    pub count: i64,
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("api error {status}: {body}")]
    Api { status: u16, body: String },
}

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

    fn url(&self, path: &str) -> String {
        format!("{}/api/v1{}", self.base_url.trim_end_matches('/'), path)
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        let resp = self
            .http
            .get(self.url(path))
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        Self::parse_response(resp).await
    }

    async fn post_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &impl Serialize,
    ) -> Result<T, ClientError> {
        let resp = self
            .http
            .post(self.url(path))
            .bearer_auth(&self.api_key)
            .json(body)
            .send()
            .await?;
        Self::parse_response(resp).await
    }

    async fn post_empty(&self, path: &str) -> Result<(), ClientError> {
        let resp = self
            .http
            .post(self.url(path))
            .bearer_auth(&self.api_key)
            .send()
            .await?;
        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            let body = resp.text().await.unwrap_or_default();
            Err(ClientError::Api {
                status: status.as_u16(),
                body,
            })
        }
    }

    async fn parse_response<T: serde::de::DeserializeOwned>(
        resp: reqwest::Response,
    ) -> Result<T, ClientError> {
        let status = resp.status();
        let body = resp.text().await?;
        if status.is_success() {
            Ok(serde_json::from_str(&body)?)
        } else {
            Err(ClientError::Api {
                status: status.as_u16(),
                body,
            })
        }
    }

    pub async fn list_inboxes(&self) -> Result<Vec<Inbox>, ClientError> {
        self.get_json("/inboxes").await
    }

    pub async fn list_messages(
        &self,
        inbox_id: Uuid,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Message>, ClientError> {
        self.get_json(&format!(
            "/inboxes/{inbox_id}/messages?limit={limit}&offset={offset}"
        ))
        .await
    }

    pub async fn get_message(&self, inbox_id: Uuid, msg_id: Uuid) -> Result<Message, ClientError> {
        self.get_json(&format!("/inboxes/{inbox_id}/messages/{msg_id}"))
            .await
    }

    pub async fn get_thread(
        &self,
        inbox_id: Uuid,
        thread_id: Uuid,
    ) -> Result<Vec<Message>, ClientError> {
        self.get_json(&format!(
            "/inboxes/{inbox_id}/messages?thread_id={thread_id}"
        ))
        .await
    }

    pub async fn search(&self, query: &str) -> Result<Vec<Message>, ClientError> {
        let encoded = urlencoding::encode(query);
        self.get_json(&format!("/search?q={encoded}")).await
    }

    pub async fn briefing(&self, period: &str) -> Result<Briefing, ClientError> {
        let encoded = urlencoding::encode(period);
        self.get_json(&format!("/briefing?period={encoded}")).await
    }

    pub async fn list_approvals(&self) -> Result<Vec<Approval>, ClientError> {
        self.get_json("/approvals").await
    }

    pub async fn approve(&self, id: Uuid) -> Result<(), ClientError> {
        self.post_empty(&format!("/approvals/{id}/approve")).await
    }

    pub async fn reject(&self, id: Uuid) -> Result<(), ClientError> {
        self.post_empty(&format!("/approvals/{id}/reject")).await
    }

    pub async fn send_message(
        &self,
        inbox_id: Uuid,
        to: &str,
        subject: &str,
        body: &str,
    ) -> Result<Message, ClientError> {
        self.post_json(
            &format!("/inboxes/{inbox_id}/messages"),
            &serde_json::json!({
                "to": [to],
                "subject": subject,
                "text_body": body,
            }),
        )
        .await
    }

    pub async fn create_draft(
        &self,
        inbox_id: Uuid,
        to: &str,
        subject: &str,
        body: &str,
    ) -> Result<serde_json::Value, ClientError> {
        self.post_json(
            &format!("/inboxes/{inbox_id}/drafts"),
            &serde_json::json!({
                "to_addrs": [to],
                "subject": subject,
                "text_body": body,
            }),
        )
        .await
    }

    pub async fn send_draft(&self, inbox_id: Uuid, draft_id: Uuid) -> Result<(), ClientError> {
        self.post_empty(&format!("/inboxes/{inbox_id}/drafts/{draft_id}/send"))
            .await
    }

    pub fn ws_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        let ws_base = if base.starts_with("https://") {
            base.replacen("https://", "wss://", 1)
        } else {
            base.replacen("http://", "ws://", 1)
        };
        let encoded_key = urlencoding::encode(&self.api_key);
        format!("{ws_base}/api/v1/ws?key={encoded_key}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client() -> PostbloxClient {
        PostbloxClient::new("http://localhost:3000".into(), "test-key".into())
    }

    #[test]
    fn test_url_construction() {
        let c = test_client();
        assert_eq!(c.url("/inboxes"), "http://localhost:3000/api/v1/inboxes");
    }

    #[test]
    fn test_url_trailing_slash_stripped() {
        let c = PostbloxClient::new("http://localhost:3000/".into(), "k".into());
        assert_eq!(c.url("/inboxes"), "http://localhost:3000/api/v1/inboxes");
    }

    #[test]
    fn test_url_nested_path() {
        let c = test_client();
        let id = "550e8400-e29b-41d4-a716-446655440000";
        assert_eq!(
            c.url(&format!("/inboxes/{id}/messages")),
            format!("http://localhost:3000/api/v1/inboxes/{id}/messages")
        );
    }

    #[test]
    fn test_ws_url_http() {
        let c = test_client();
        assert_eq!(c.ws_url(), "ws://localhost:3000/api/v1/ws?key=test-key");
    }

    #[test]
    fn test_ws_url_https() {
        let c = PostbloxClient::new("https://mail.example.com".into(), "key123".into());
        assert_eq!(c.ws_url(), "wss://mail.example.com/api/v1/ws?key=key123");
    }

    #[test]
    fn test_ws_url_trailing_slash() {
        let c = PostbloxClient::new("http://localhost:3000/".into(), "k".into());
        assert_eq!(c.ws_url(), "ws://localhost:3000/api/v1/ws?key=k");
    }

    #[test]
    fn test_ws_url_encodes_special_chars() {
        let c = PostbloxClient::new(
            "http://localhost:3000".into(),
            "key+with=special&chars".into(),
        );
        assert_eq!(
            c.ws_url(),
            "ws://localhost:3000/api/v1/ws?key=key%2Bwith%3Dspecial%26chars"
        );
    }

    #[test]
    fn test_get_thread_uses_messages_endpoint() {
        let c = test_client();
        let inbox_id = "550e8400-e29b-41d4-a716-446655440000";
        let thread_id = "660e8400-e29b-41d4-a716-446655440000";
        let expected = format!(
            "http://localhost:3000/api/v1/inboxes/{inbox_id}/messages?thread_id={thread_id}"
        );
        assert_eq!(
            c.url(&format!(
                "/inboxes/{inbox_id}/messages?thread_id={thread_id}"
            )),
            expected
        );
    }
}
