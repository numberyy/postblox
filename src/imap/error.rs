//! Errors for the IMAP transport.

use thiserror::Error;

/// Error returned by IMAP transport operations.
#[derive(Debug, Error)]
pub enum ImapError {
    /// Underlying TCP/stream IO failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// TLS handshake or configuration failed.
    #[error("tls: {0}")]
    Tls(String),

    /// IMAP protocol-level failure (greeting, command, or response decode).
    #[error("imap protocol: {0}")]
    Protocol(String),

    /// Server rejected `LOGIN` or `AUTHENTICATE`.
    #[error("auth failed: {0}")]
    Auth(String),

    /// Required IMAP capability is not advertised by the server.
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// `host` is not a valid TLS server name.
    #[error("invalid server name: {0}")]
    InvalidName(String),
}

impl From<async_imap::error::Error> for ImapError {
    fn from(e: async_imap::error::Error) -> Self {
        // Surface the well-known auth-vs-other split so callers can
        // distinguish bad creds from network/server issues.
        let msg = e.to_string();
        let lower = msg.to_lowercase();
        if lower.contains("authentication") || lower.contains("authenticationfailed") {
            ImapError::Auth(msg)
        } else {
            ImapError::Protocol(msg)
        }
    }
}
