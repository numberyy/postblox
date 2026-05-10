//! SMTP submission per account.
//!
//! [`SmtpSubmitter`] is the trait the daemon depends on; the concrete
//! [`LettreSmtpSubmitter`] backs it with `lettre`'s async transport.
//! TLS handling is explicit: implicit TLS via `smtp_use_tls` and
//! STARTTLS via `smtp_starttls` are mutually exclusive and validated
//! up-front. Both `AUTH PLAIN`/`LOGIN` and `XOAUTH2` SASL flows are
//! supported, picked from [`crate::auth::CredentialKind`]. Errors
//! collapse `lettre` failures into a small [`SmtpError`] taxonomy so
//! the daemon can decide retry vs. surface-to-user.

use std::time::Duration;

use lettre::address::{Address, Envelope};
use lettre::transport::smtp::authentication::{Credentials, Mechanism};
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use thiserror::Error;

use crate::auth::{CredentialKind, MailCredential};

/// SMTP server endpoint plus transport-security flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpServer {
    /// Submission server hostname.
    pub host: String,
    /// Submission server port.
    pub port: u16,
    /// Use implicit TLS on connect (mutually exclusive with `starttls`).
    pub use_tls: bool,
    /// Upgrade an unencrypted connection via `STARTTLS`.
    pub starttls: bool,
}

/// All inputs for a single SMTP message submission.
pub struct SmtpSubmitRequest {
    /// SMTP server endpoint and transport security.
    pub server: SmtpServer,
    /// SASL username used to authenticate.
    pub username: String,
    /// Credential to authenticate with (password or OAuth2 bearer).
    pub credential: MailCredential,
    /// `MAIL FROM` envelope sender.
    pub from: String,
    /// `RCPT TO` envelope recipients.
    pub recipients: Vec<String>,
    /// Raw RFC 5322 message bytes (DATA payload).
    pub mime: Vec<u8>,
}

/// Error returned by [`SmtpSubmitter`] implementations.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SmtpError {
    /// Server rejected the SASL credentials.
    #[error("auth failed: {0}")]
    Auth(String),

    /// Caller-supplied addresses or envelope were invalid.
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    /// Server config was contradictory (e.g. both implicit TLS and STARTTLS).
    #[error("invalid config: {0}")]
    InvalidConfig(String),

    /// Retryable transport failure (timeout, 4xx, network blip).
    #[error("transient: {0}")]
    Transient(String),

    /// Any other lettre / SMTP failure not covered by the variants above.
    #[error("internal: {0}")]
    Internal(String),
}

/// Async SMTP submission trait used by the daemon.
#[async_trait::async_trait]
pub trait SmtpSubmitter: Send + Sync {
    /// Submit a single message via SMTP using the credentials and
    /// transport security in `request`.
    ///
    /// # Errors
    ///
    /// Returns:
    /// - [`SmtpError::InvalidConfig`] if the server config is contradictory
    ///   (for example both `use_tls` and `starttls` set) or TLS parameters
    ///   cannot be built.
    /// - [`SmtpError::InvalidRequest`] if `from`/`recipients` cannot be parsed
    ///   into a valid envelope.
    /// - [`SmtpError::Auth`] if the server rejects the credentials
    ///   (`535 Authentication failed` or equivalent SASL refusal).
    /// - [`SmtpError::Transient`] for retryable transport errors
    ///   (timeouts, 4xx responses, network blips).
    /// - [`SmtpError::Internal`] for any other lettre / SMTP failure.
    async fn submit(&self, request: SmtpSubmitRequest) -> Result<(), SmtpError>;
}

/// Production [`SmtpSubmitter`] backed by `lettre`'s async transport.
#[derive(Debug, Default)]
pub struct LettreSmtpSubmitter;

impl LettreSmtpSubmitter {
    /// Construct a new lettre-backed submitter.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl SmtpSubmitter for LettreSmtpSubmitter {
    async fn submit(&self, request: SmtpSubmitRequest) -> Result<(), SmtpError> {
        let envelope = build_envelope(&request.from, &request.recipients)?;
        let mailer = build_transport(&request)?;
        mailer
            .send_raw(&envelope, &request.mime)
            .await
            .map_err(map_lettre_error)?;
        mailer.shutdown().await;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SmtpSecurity {
    Wrapper,
    StartTls,
    None,
}

fn security_for(server: &SmtpServer) -> Result<SmtpSecurity, SmtpError> {
    match (server.use_tls, server.starttls) {
        (true, false) => Ok(SmtpSecurity::Wrapper),
        (false, true) => Ok(SmtpSecurity::StartTls),
        (false, false) => Ok(SmtpSecurity::None),
        (true, true) => Err(SmtpError::InvalidConfig(
            "smtp_use_tls and smtp_starttls cannot both be true".into(),
        )),
    }
}

fn build_envelope(from: &str, recipients: &[String]) -> Result<Envelope, SmtpError> {
    let from = from
        .parse::<Address>()
        .map_err(|e| SmtpError::InvalidRequest(format!("bad from address: {e}")))?;
    let to = recipients
        .iter()
        .map(|addr| {
            addr.parse::<Address>()
                .map_err(|e| SmtpError::InvalidRequest(format!("bad recipient address: {e}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Envelope::new(Some(from), to)
        .map_err(|e| SmtpError::InvalidRequest(format!("bad envelope: {e}")))
}

fn build_transport(
    request: &SmtpSubmitRequest,
) -> Result<AsyncSmtpTransport<Tokio1Executor>, SmtpError> {
    let tls = match security_for(&request.server)? {
        SmtpSecurity::Wrapper => Some(Tls::Wrapper(tls_parameters(&request.server.host)?)),
        SmtpSecurity::StartTls => Some(Tls::Required(tls_parameters(&request.server.host)?)),
        SmtpSecurity::None => Some(Tls::None),
    };

    let credentials = Credentials::new(
        request.username.clone(),
        request.credential.secret().to_string(),
    );
    let mut builder =
        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(request.server.host.clone())
            .port(request.server.port)
            .timeout(Some(Duration::from_secs(30)))
            .credentials(credentials);
    if request.credential.kind() == CredentialKind::OAuth2Bearer {
        builder = builder.authentication(vec![Mechanism::Xoauth2]);
    }

    if let Some(tls) = tls {
        builder = builder.tls(tls);
    }

    Ok(builder.build())
}

fn tls_parameters(host: &str) -> Result<TlsParameters, SmtpError> {
    TlsParameters::new(host.to_string()).map_err(|e| SmtpError::InvalidConfig(e.to_string()))
}

fn map_lettre_error(err: lettre::transport::smtp::Error) -> SmtpError {
    let message = err.to_string();
    let lower = message.to_ascii_lowercase();
    let status = err.status().map(u16::from);
    if status == Some(535)
        || lower.contains("auth")
        || lower.contains("credential")
        || lower.contains("password")
    {
        SmtpError::Auth(message)
    } else if err.is_transient() || err.is_timeout() {
        SmtpError::Transient(message)
    } else {
        SmtpError::Internal(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(use_tls: bool, starttls: bool) -> SmtpServer {
        SmtpServer {
            host: "smtp.example.com".into(),
            port: 587,
            use_tls,
            starttls,
        }
    }

    #[test]
    fn test_security_mode_uses_tls_wrapper_for_implicit_tls() {
        assert_eq!(
            security_for(&server(true, false)).unwrap(),
            SmtpSecurity::Wrapper
        );
    }

    #[test]
    fn test_security_mode_uses_starttls_when_requested() {
        assert_eq!(
            security_for(&server(false, true)).unwrap(),
            SmtpSecurity::StartTls
        );
    }

    #[test]
    fn test_security_mode_allows_plain_smtp_when_configured() {
        assert_eq!(
            security_for(&server(false, false)).unwrap(),
            SmtpSecurity::None
        );
    }

    #[test]
    fn test_security_mode_rejects_conflicting_tls_flags() {
        assert!(matches!(
            security_for(&server(true, true)),
            Err(SmtpError::InvalidConfig(_))
        ));
    }

    #[test]
    fn test_build_envelope_rejects_missing_recipients() {
        assert!(matches!(
            build_envelope("sender@example.com", &[]),
            Err(SmtpError::InvalidRequest(_))
        ));
    }

    #[test]
    fn test_build_envelope_accepts_bcc_only_recipient() {
        let envelope = build_envelope("sender@example.com", &["blind@example.com".into()]).unwrap();
        assert_eq!(envelope.to().len(), 1);
        assert_eq!(envelope.from().unwrap().to_string(), "sender@example.com");
    }
}
