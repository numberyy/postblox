//! Gmail OAuth2 + XOAUTH2 SASL helpers.
//!
//! The full Gmail authentication flow lives here: building the
//! authorization URL ([`authorization_url`]), exchanging or refreshing
//! tokens through the [`GoogleOAuth`] trait (and the production
//! [`GoogleOAuthHttpClient`] impl), persisting the result through
//! [`crate::secrets::SecretStore`] via [`load_stored_oauth`] /
//! [`store_stored_oauth`], and finally formatting a Gmail-shaped
//! XOAUTH2 SASL string for IMAP/SMTP via [`xoauth2_sasl_string`].
//!
//! Confidential fields ([`GoogleOAuthConfig::client_secret`],
//! [`GoogleOAuthToken::access_token`], [`GoogleOAuthToken::refresh_token`])
//! have hand-written `Debug` impls that print `<redacted>` so
//! `tracing` and panic backtraces never leak secrets. Token bytes are
//! also held in `zeroize::Zeroizing` buffers when they cross the
//! reqwest boundary.
//!
//! Configuration is locked to the Gmail scope (`GMAIL_SCOPE`); other
//! Google APIs are deliberately rejected at validate time so an
//! over-scoped token can't slip through.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::Duration as StdDuration;
use thiserror::Error;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::secrets::{SecretError, SecretStore};

const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
pub const GMAIL_SCOPE: &str = "https://mail.google.com/";
pub const DEFAULT_REQUEST_TIMEOUT: StdDuration = StdDuration::from_secs(10);
const REFRESH_SKEW_SECONDS: i64 = 60;

#[derive(Clone, PartialEq, Eq)]
pub struct GoogleOAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
}

impl std::fmt::Debug for GoogleOAuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleOAuthConfig")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("redirect_uri", &self.redirect_uri)
            .field("scopes", &self.scopes)
            .finish()
    }
}

impl GoogleOAuthConfig {
    pub fn gmail(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            redirect_uri: redirect_uri.into(),
            scopes: vec![GMAIL_SCOPE.into()],
        }
    }

    fn validate_for_auth_url(&self, state: &str) -> Result<(), GoogleOAuthError> {
        validate_non_empty("client_id", &self.client_id)?;
        validate_non_empty("redirect_uri", &self.redirect_uri)?;
        validate_non_empty("state", state)?;
        self.validate_gmail_scope()
    }

    fn validate_for_token_request(&self) -> Result<(), GoogleOAuthError> {
        validate_non_empty("client_id", &self.client_id)?;
        validate_non_empty("client_secret", &self.client_secret)?;
        validate_non_empty("redirect_uri", &self.redirect_uri)?;
        self.validate_gmail_scope()
    }

    fn validate_gmail_scope(&self) -> Result<(), GoogleOAuthError> {
        if self.scopes.len() == 1 && self.scopes[0] == GMAIL_SCOPE {
            return Ok(());
        }
        Err(GoogleOAuthError::InvalidInput(
            "only the Gmail OAuth scope is supported".into(),
        ))
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoogleOAuthToken {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
    pub token_type: String,
    pub scope: Option<String>,
}

impl std::fmt::Debug for GoogleOAuthToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleOAuthToken")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .field("token_type", &self.token_type)
            .field("scope", &self.scope)
            .finish()
    }
}

impl GoogleOAuthToken {
    pub fn needs_refresh(&self, now: DateTime<Utc>) -> bool {
        self.expires_at <= now + Duration::seconds(REFRESH_SKEW_SECONDS)
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredGoogleOAuth {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
    pub token: GoogleOAuthToken,
}

impl std::fmt::Debug for StoredGoogleOAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredGoogleOAuth")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("redirect_uri", &self.redirect_uri)
            .field("scopes", &self.scopes)
            .field("token", &self.token)
            .finish()
    }
}

impl StoredGoogleOAuth {
    pub fn new(config: GoogleOAuthConfig, token: GoogleOAuthToken) -> Self {
        Self {
            client_id: config.client_id,
            client_secret: config.client_secret,
            redirect_uri: config.redirect_uri,
            scopes: config.scopes,
            token,
        }
    }

    pub fn config(&self) -> GoogleOAuthConfig {
        GoogleOAuthConfig {
            client_id: self.client_id.clone(),
            client_secret: self.client_secret.clone(),
            redirect_uri: self.redirect_uri.clone(),
            scopes: self.scopes.clone(),
        }
    }
}

#[derive(Debug, Error)]
pub enum GoogleOAuthError {
    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("oauth response did not include refresh token")]
    MissingRefreshToken,

    #[error("oauth token endpoint returned status {0}")]
    HttpStatus(u16),

    #[error("http: {0}")]
    Http(#[from] reqwest::Error),

    #[error("secret store: {0}")]
    Secret(#[from] SecretError),

    #[error("decode: {0}")]
    Decode(String),
}

#[async_trait::async_trait]
pub trait GoogleOAuth: Send + Sync {
    async fn exchange_code(
        &self,
        config: &GoogleOAuthConfig,
        code: &str,
    ) -> Result<GoogleOAuthToken, GoogleOAuthError>;

    async fn refresh_token(
        &self,
        config: &GoogleOAuthConfig,
        token: &GoogleOAuthToken,
    ) -> Result<GoogleOAuthToken, GoogleOAuthError>;
}

#[derive(Clone)]
pub struct GoogleOAuthHttpClient {
    http: reqwest::Client,
    token_endpoint: String,
}

impl Default for GoogleOAuthHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl GoogleOAuthHttpClient {
    pub fn new() -> Self {
        // Production constructor: every caller shares one connection-pooled
        // `reqwest::Client` via the module-scoped `OnceLock` (E-M1). The
        // test-only constructors below stay isolated so they can apply
        // custom timeouts/endpoints without contaminating the cached client.
        Self {
            http: shared_http_client().clone(),
            token_endpoint: TOKEN_ENDPOINT.into(),
        }
    }

    pub fn with_token_endpoint(token_endpoint: impl Into<String>) -> Self {
        Self::with_token_endpoint_and_timeout(token_endpoint, DEFAULT_REQUEST_TIMEOUT)
    }

    pub fn with_token_endpoint_and_timeout(
        token_endpoint: impl Into<String>,
        timeout: StdDuration,
    ) -> Self {
        Self {
            http: bounded_http_client(timeout),
            token_endpoint: token_endpoint.into(),
        }
    }

    async fn post_token_form(
        &self,
        form: &[(&str, &str)],
    ) -> Result<TokenEndpointResponse, GoogleOAuthError> {
        let response = self
            .http
            .post(&self.token_endpoint)
            .form(form)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(GoogleOAuthError::HttpStatus(status.as_u16()));
        }
        Ok(response.json::<TokenEndpointResponse>().await?)
    }
}

#[async_trait::async_trait]
impl GoogleOAuth for GoogleOAuthHttpClient {
    async fn exchange_code(
        &self,
        config: &GoogleOAuthConfig,
        code: &str,
    ) -> Result<GoogleOAuthToken, GoogleOAuthError> {
        config.validate_for_token_request()?;
        validate_non_empty("code", code)?;
        let response = self
            .post_token_form(&[
                ("client_id", config.client_id.as_str()),
                ("client_secret", config.client_secret.as_str()),
                ("code", code),
                ("grant_type", "authorization_code"),
                ("redirect_uri", config.redirect_uri.as_str()),
            ])
            .await?;
        response.into_token(None)
    }

    async fn refresh_token(
        &self,
        config: &GoogleOAuthConfig,
        token: &GoogleOAuthToken,
    ) -> Result<GoogleOAuthToken, GoogleOAuthError> {
        config.validate_for_token_request()?;
        validate_non_empty("refresh_token", &token.refresh_token)?;
        let response = self
            .post_token_form(&[
                ("client_id", config.client_id.as_str()),
                ("client_secret", config.client_secret.as_str()),
                ("grant_type", "refresh_token"),
                ("refresh_token", token.refresh_token.as_str()),
            ])
            .await?;
        response.into_token(Some(token.refresh_token.as_str()))
    }
}

#[derive(Deserialize)]
struct TokenEndpointResponse {
    access_token: String,
    expires_in: i64,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default = "default_token_type")]
    token_type: String,
    #[serde(default)]
    scope: Option<String>,
}

impl TokenEndpointResponse {
    fn into_token(
        self,
        existing_refresh_token: Option<&str>,
    ) -> Result<GoogleOAuthToken, GoogleOAuthError> {
        validate_non_empty("access_token", &self.access_token)?;
        if self.expires_in <= 0 {
            return Err(GoogleOAuthError::InvalidInput(
                "expires_in must be positive".into(),
            ));
        }
        let refresh_token = match (self.refresh_token, existing_refresh_token) {
            (Some(refresh_token), _) if !refresh_token.is_empty() => refresh_token,
            (_, Some(refresh_token)) if !refresh_token.is_empty() => refresh_token.to_string(),
            _ => return Err(GoogleOAuthError::MissingRefreshToken),
        };
        Ok(GoogleOAuthToken {
            access_token: self.access_token,
            refresh_token,
            expires_at: Utc::now() + Duration::seconds(self.expires_in),
            token_type: self.token_type,
            scope: self.scope,
        })
    }
}

fn default_token_type() -> String {
    "Bearer".into()
}

fn bounded_http_client(timeout: StdDuration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .expect("BUG: reqwest client builder accepts plain timeout config")
}

// Module-scoped cache so all production `GoogleOAuthHttpClient::new()` call
// sites share one connection pool (E-M1). `reqwest::Client` is internally
// reference-counted, so cloning the cached handle is cheap.
fn shared_http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| bounded_http_client(DEFAULT_REQUEST_TIMEOUT))
}

pub fn authorization_url(
    config: &GoogleOAuthConfig,
    state: &str,
) -> Result<String, GoogleOAuthError> {
    config.validate_for_auth_url(state)?;
    let scope = config.scopes.join(" ");
    let params = [
        ("client_id", config.client_id.as_str()),
        ("redirect_uri", config.redirect_uri.as_str()),
        ("response_type", "code"),
        ("scope", scope.as_str()),
        ("state", state),
        ("access_type", "offline"),
        ("prompt", "consent"),
    ];
    let query = params
        .iter()
        .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    Ok(format!("{AUTH_ENDPOINT}?{query}"))
}

pub fn xoauth2_sasl_string(username: &str, access_token: &str) -> String {
    format!("user={username}\x01auth=Bearer {access_token}\x01\x01")
}

pub async fn load_stored_oauth(
    secrets: &dyn SecretStore,
    account_id: Uuid,
) -> Result<Option<StoredGoogleOAuth>, GoogleOAuthError> {
    let Some(secret) = secrets.get(account_id).await? else {
        return Ok(None);
    };
    serde_json::from_str(secret.as_str())
        .map(Some)
        .map_err(|e| GoogleOAuthError::Decode(format!("stored oauth json: {e}")))
}

pub async fn store_stored_oauth(
    secrets: &dyn SecretStore,
    account_id: Uuid,
    stored: &StoredGoogleOAuth,
) -> Result<(), GoogleOAuthError> {
    let json = serde_json::to_string(stored)
        .map_err(|e| GoogleOAuthError::Decode(format!("stored oauth json: {e}")))?;
    secrets.put(account_id, Zeroizing::new(json)).await?;
    Ok(())
}

fn validate_non_empty(field: &str, value: &str) -> Result<(), GoogleOAuthError> {
    if value.trim().is_empty() {
        Err(GoogleOAuthError::InvalidInput(format!(
            "{field} must be non-empty"
        )))
    } else {
        Ok(())
    }
}

fn percent_encode(input: &str) -> String {
    let mut out = String::new();
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(b));
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::file::{FileSecretStore, KdfParams};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn config() -> GoogleOAuthConfig {
        GoogleOAuthConfig::gmail("client id", "client secret", "http://127.0.0.1/callback")
    }

    #[test]
    fn test_authorization_url_encodes_google_params_and_state() {
        let url = authorization_url(&config(), "state/with space").unwrap();
        assert!(url.starts_with(AUTH_ENDPOINT));
        assert!(url.contains("client_id=client%20id"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%2Fcallback"));
        assert!(url.contains("scope=https%3A%2F%2Fmail.google.com%2F"));
        assert!(url.contains("state=state%2Fwith%20space"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("prompt=consent"));
        assert!(!url.contains("include_granted_scopes"));
    }

    #[test]
    fn test_authorization_url_rejects_empty_state() {
        let err = authorization_url(&config(), " ").unwrap_err();
        assert!(matches!(err, GoogleOAuthError::InvalidInput(_)));
    }

    #[test]
    fn test_authorization_url_rejects_non_gmail_scope() {
        let mut config = config();
        config.scopes = vec!["openid".into()];
        let err = authorization_url(&config, "state").unwrap_err();
        assert!(matches!(err, GoogleOAuthError::InvalidInput(_)));
    }

    #[test]
    fn test_oauth_debug_redacts_secret_material() {
        let mut config = config();
        config.client_secret = "client-secret-value".into();
        let token = GoogleOAuthToken {
            access_token: "access-secret-value".into(),
            refresh_token: "refresh-secret-value".into(),
            expires_at: Utc::now() + Duration::seconds(3600),
            token_type: "Bearer".into(),
            scope: None,
        };
        let stored = StoredGoogleOAuth::new(config.clone(), token.clone());
        let printed = format!("{config:?} {token:?} {stored:?}");

        assert!(printed.contains("<redacted>"));
        assert!(!printed.contains("client-secret-value"));
        assert!(!printed.contains("access-secret-value"));
        assert!(!printed.contains("refresh-secret-value"));
    }

    #[test]
    fn test_shared_http_client_returns_same_handle_across_calls() {
        // E-M1: every production `GoogleOAuthHttpClient::new()` must reuse
        // the same module-scoped client so the connection pool is shared.
        let first = shared_http_client() as *const reqwest::Client;
        let second = shared_http_client() as *const reqwest::Client;
        assert!(std::ptr::eq(first, second));
    }

    #[test]
    fn test_xoauth2_sasl_string_matches_gmail_format() {
        assert_eq!(
            xoauth2_sasl_string("me@example.com", "ya29.token"),
            "user=me@example.com\x01auth=Bearer ya29.token\x01\x01"
        );
    }

    #[test]
    fn test_token_needs_refresh_before_expiry_skew() {
        let now = Utc::now();
        let token = GoogleOAuthToken {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: now + Duration::seconds(30),
            token_type: "Bearer".into(),
            scope: None,
        };
        assert!(token.needs_refresh(now));

        let token = GoogleOAuthToken {
            expires_at: now + Duration::seconds(120),
            ..token
        };
        assert!(!token.needs_refresh(now));
    }

    #[tokio::test]
    async fn test_code_exchange_posts_form_and_requires_code() {
        let client = GoogleOAuthHttpClient::with_token_endpoint("http://127.0.0.1:9/token");
        let err = client.exchange_code(&config(), "").await.unwrap_err();
        assert!(matches!(err, GoogleOAuthError::InvalidInput(_)));

        let (endpoint, request) = serve_once(
            r#"{"access_token":"access","refresh_token":"refresh","expires_in":3600,"token_type":"Bearer","scope":"https://mail.google.com/"}"#,
        )
        .await;
        let client = GoogleOAuthHttpClient::with_token_endpoint(endpoint);
        let token = client.exchange_code(&config(), "code-123").await.unwrap();
        assert_eq!(token.access_token, "access");
        assert_eq!(token.refresh_token, "refresh");
        let request = request.await.unwrap();
        assert!(request.contains("POST /token HTTP/1.1"));
        assert!(request.contains("grant_type=authorization_code"));
        assert!(request.contains("code=code-123"));
        assert!(request.contains("client_id=client+id"));
    }

    #[tokio::test]
    async fn test_refresh_keeps_existing_refresh_token_when_google_omits_it() {
        let (endpoint, request) =
            serve_once(r#"{"access_token":"fresh","expires_in":3600,"token_type":"Bearer"}"#).await;
        let client = GoogleOAuthHttpClient::with_token_endpoint(endpoint);
        let old = GoogleOAuthToken {
            access_token: "old".into(),
            refresh_token: "refresh-old".into(),
            expires_at: Utc::now() - Duration::seconds(1),
            token_type: "Bearer".into(),
            scope: None,
        };

        let token = client.refresh_token(&config(), &old).await.unwrap();
        assert_eq!(token.access_token, "fresh");
        assert_eq!(token.refresh_token, "refresh-old");
        let request = request.await.unwrap();
        assert!(request.contains("grant_type=refresh_token"));
        assert!(request.contains("refresh_token=refresh-old"));
    }

    #[tokio::test]
    async fn test_token_request_uses_bounded_timeout() {
        let (endpoint, handle) = serve_without_response().await;
        let client = GoogleOAuthHttpClient::with_token_endpoint_and_timeout(
            endpoint,
            StdDuration::from_millis(20),
        );

        let err = tokio::time::timeout(
            StdDuration::from_secs(1),
            client.exchange_code(&config(), "code-123"),
        )
        .await
        .unwrap()
        .unwrap_err();
        handle.abort();

        assert!(matches!(err, GoogleOAuthError::Http(err) if err.is_timeout()));
    }

    #[tokio::test]
    async fn test_store_load_oauth_secret_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSecretStore::with_params(
            dir.path().join("secrets.bin"),
            "test",
            KdfParams::insecure_for_tests(),
        );
        let id = Uuid::new_v4();
        let stored = StoredGoogleOAuth::new(
            config(),
            GoogleOAuthToken {
                access_token: "access".into(),
                refresh_token: "refresh".into(),
                expires_at: Utc::now() + Duration::seconds(3600),
                token_type: "Bearer".into(),
                scope: None,
            },
        );

        store_stored_oauth(&store, id, &stored).await.unwrap();
        let loaded = load_stored_oauth(&store, id).await.unwrap().unwrap();
        assert_eq!(loaded, stored);
    }

    async fn serve_once(body: &'static str) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = stream.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            request
        });
        (format!("http://{addr}/token"), handle)
    }

    async fn serve_without_response() -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = stream.read(&mut buf).await;
            tokio::time::sleep(StdDuration::from_secs(5)).await;
        });
        (format!("http://{addr}/token"), handle)
    }
}
