use std::time::Duration;

use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, Tokio1Executor};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StalwartError {
    #[error("stalwart request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("stalwart returned {status}: {body}")]
    Api { status: u16, body: String },
    #[error("smtp error: {0}")]
    Smtp(String),
}

#[derive(Clone)]
pub struct StalwartClient {
    http: reqwest::Client,
    base_url: String,
    admin_user: String,
    admin_token: String,
    smtp_transport: AsyncSmtpTransport<Tokio1Executor>,
}

impl StalwartClient {
    pub fn new(
        base_url: &str,
        admin_user: &str,
        admin_token: &str,
        smtp_host: Option<&str>,
        smtp_port: Option<u16>,
    ) -> Result<Self, StalwartError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()?;

        let parsed_host = smtp_host.map(String::from).unwrap_or_else(|| {
            // Extract host from HTTP base_url (e.g., "http://stalwart:8080" → "stalwart")
            base_url
                .trim_start_matches("http://")
                .trim_start_matches("https://")
                .split(':')
                .next()
                .unwrap_or("localhost")
                .to_string()
        });

        let smtp_transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&parsed_host)
            .port(smtp_port.unwrap_or(25))
            .credentials(Credentials::new(
                admin_user.to_string(),
                admin_token.to_string(),
            ))
            .timeout(Some(Duration::from_secs(30)))
            .build();

        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            admin_user: admin_user.to_string(),
            admin_token: admin_token.to_string(),
            smtp_transport,
        })
    }

    async fn check_response(resp: reqwest::Response) -> Result<reqwest::Response, StalwartError> {
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp
                .text()
                .await
                .unwrap_or_else(|e| format!("(body read failed: {e})"));
            return Err(StalwartError::Api { status, body });
        }
        Ok(resp)
    }

    pub async fn create_account(&self, email: &str, password: &str) -> Result<(), StalwartError> {
        let resp = self
            .http
            .post(format!("{}/api/principal", self.base_url))
            .basic_auth(&self.admin_user, Some(&self.admin_token))
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
            .basic_auth(&self.admin_user, Some(&self.admin_token))
            .send()
            .await?;

        Self::check_response(resp).await?;
        Ok(())
    }

    pub async fn create_domain(&self, name: &str) -> Result<String, StalwartError> {
        let resp = self
            .http
            .post(format!("{}/api/principal", self.base_url))
            .basic_auth(&self.admin_user, Some(&self.admin_token))
            .json(&serde_json::json!({
                "type": "domain",
                "name": name,
            }))
            .send()
            .await?;

        let resp = Self::check_response(resp).await?;
        let body: serde_json::Value = resp.json().await?;
        let principal_id = match body["data"]["id"].as_str() {
            Some(id) => id.to_string(),
            None => {
                tracing::warn!("stalwart create_domain response missing data.id, using domain name as principal_id");
                name.to_string()
            }
        };
        Ok(principal_id)
    }

    pub async fn delete_domain(&self, principal_id: &str) -> Result<(), StalwartError> {
        let resp = self
            .http
            .delete(format!("{}/api/principal/{principal_id}", self.base_url))
            .basic_auth(&self.admin_user, Some(&self.admin_token))
            .send()
            .await?;

        Self::check_response(resp).await?;
        Ok(())
    }

    pub async fn get_dns_records(&self, domain: &str) -> Result<serde_json::Value, StalwartError> {
        let resp = self
            .http
            .get(format!("{}/api/dns/records/{domain}", self.base_url))
            .basic_auth(&self.admin_user, Some(&self.admin_token))
            .send()
            .await?;

        let resp = Self::check_response(resp).await?;
        let body: serde_json::Value = resp.json().await?;
        Ok(body)
    }

    pub async fn set_settings(&self, settings: &[(&str, &str)]) -> Result<(), StalwartError> {
        let values: Vec<(String, String)> = settings
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let payload = serde_json::json!([{
            "type": "insert",
            "values": values,
            "assertEmpty": false,
        }]);

        let resp = self
            .http
            .post(format!("{}/api/settings", self.base_url))
            .basic_auth(&self.admin_user, Some(&self.admin_token))
            .json(&payload)
            .send()
            .await?;

        Self::check_response(resp).await?;
        Ok(())
    }

    pub async fn configure_mta_hook(
        &self,
        postblox_url: &str,
        token: &str,
    ) -> Result<(), StalwartError> {
        let hook_url = format!("{postblox_url}/internal/stalwart/mta-hook");
        self.set_settings(&[
            ("session.hook.postblox.enable", "true"),
            ("session.hook.postblox.url", &hook_url),
            ("session.hook.postblox.stages", "[\"data\"]"),
            ("session.hook.postblox.auth.username", "postblox"),
            ("session.hook.postblox.auth.secret", token),
            ("session.hook.postblox.options.tempfail-on-error", "true"),
        ])
        .await
    }

    pub async fn configure_relay(
        &self,
        host: &str,
        port: u16,
        username: Option<&str>,
        password: Option<&str>,
        starttls: bool,
    ) -> Result<(), StalwartError> {
        let port_str = port.to_string();
        let tls_val = if starttls { "starttls" } else { "true" };
        let mut settings: Vec<(&str, &str)> = vec![
            ("queue.outbound.relay.host", host),
            ("queue.outbound.relay.port", &port_str),
            ("queue.outbound.relay.tls", tls_val),
        ];
        if let Some(u) = username {
            settings.push(("queue.outbound.relay.auth.username", u));
        }
        if let Some(p) = password {
            settings.push(("queue.outbound.relay.auth.secret", p));
        }
        self.set_settings(&settings).await
    }

    pub async fn submit_message(
        &self,
        from: &str,
        to: &[&str],
        raw_mime: Vec<u8>,
    ) -> Result<(), StalwartError> {
        use lettre::AsyncTransport;

        let from_addr: lettre::Address = from
            .parse()
            .map_err(|e| StalwartError::Smtp(format!("invalid from address: {e}")))?;
        let to_addrs: Vec<lettre::Address> = to
            .iter()
            .map(|a| {
                a.parse()
                    .map_err(|e| StalwartError::Smtp(format!("invalid to address '{a}': {e}")))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let envelope = lettre::address::Envelope::new(Some(from_addr), to_addrs)
            .map_err(|e| StalwartError::Smtp(format!("invalid envelope: {e}")))?;

        self.smtp_transport
            .send_raw(&envelope, &raw_mime)
            .await
            .map_err(|e| StalwartError::Smtp(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_stalwart_client_trims_trailing_slash() {
        let client =
            StalwartClient::new("http://localhost:8080/", "admin", "token", None, None).unwrap();
        assert_eq!(client.base_url, "http://localhost:8080");
    }

    #[tokio::test]
    async fn test_stalwart_client_no_trailing_slash() {
        let client =
            StalwartClient::new("http://localhost:8080", "admin", "token", None, None).unwrap();
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

    #[tokio::test]
    async fn test_stalwart_create_domain_url() {
        let client =
            StalwartClient::new("http://localhost:8080", "admin", "token", None, None).unwrap();
        let url = format!("{}/api/principal", client.base_url);
        assert_eq!(url, "http://localhost:8080/api/principal");
    }

    #[tokio::test]
    async fn test_stalwart_delete_domain_url() {
        let client =
            StalwartClient::new("http://localhost:8080/", "admin", "token", None, None).unwrap();
        let url = format!("{}/api/principal/{}", client.base_url, "domain-123");
        assert_eq!(url, "http://localhost:8080/api/principal/domain-123");
    }

    #[tokio::test]
    async fn test_stalwart_dns_records_url() {
        let client =
            StalwartClient::new("http://localhost:8080", "admin", "token", None, None).unwrap();
        let url = format!("{}/api/dns/records/{}", client.base_url, "example.com");
        assert_eq!(url, "http://localhost:8080/api/dns/records/example.com");
    }

    #[tokio::test]
    async fn test_smtp_host_from_base_url_constructs() {
        StalwartClient::new("http://mail.example.com:8080", "admin", "tok", None, None).unwrap();
    }

    #[tokio::test]
    async fn test_smtp_host_explicit_override_constructs() {
        StalwartClient::new(
            "http://localhost:8080",
            "admin",
            "tok",
            Some("smtp.local"),
            Some(587),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn test_smtp_host_https_stripped_constructs() {
        StalwartClient::new(
            "https://stalwart.prod.internal:443",
            "admin",
            "tok",
            None,
            None,
        )
        .unwrap();
    }

    #[tokio::test]
    async fn test_stalwart_settings_url() {
        let client =
            StalwartClient::new("http://localhost:8080", "admin", "token", None, None).unwrap();
        let url = format!("{}/api/settings", client.base_url);
        assert_eq!(url, "http://localhost:8080/api/settings");
    }

    #[tokio::test]
    #[ignore] // requires running Stalwart server + STALWART_ADMIN_TOKEN env
    async fn test_stalwart_create_and_delete_account() {
        let token = std::env::var("STALWART_ADMIN_TOKEN")
            .expect("STALWART_ADMIN_TOKEN must be set for stalwart integration tests");
        let client =
            StalwartClient::new("http://localhost:8080", "admin", &token, None, None).unwrap();
        let email = format!("test-{}@postblox.local", uuid::Uuid::new_v4());
        client.create_account(&email, "password123").await.unwrap();
        client.delete_account(&email).await.unwrap();
    }
}
