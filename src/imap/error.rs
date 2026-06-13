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

    /// A stored port value did not fit in a `u16`.
    #[error("invalid port: {0}")]
    InvalidPort(i64),

    /// A network phase (connect, TLS, login, or select) exceeded its
    /// deadline. Distinct from [`ImapError::Io`] so a wedged server is
    /// retried as a transient fault rather than terminating the worker.
    #[error("timed out: {0}")]
    Timeout(String),
}

impl From<async_imap::error::Error> for ImapError {
    fn from(e: async_imap::error::Error) -> Self {
        // Surface the auth-vs-other split so callers can distinguish bad
        // creds (permanent: stop the worker) from network/server issues
        // (transient: retry). "authenticat" matches both "authentication"
        // and "AuthenticationFailed" without the redundant second check.
        let msg = e.to_string();
        if msg.to_lowercase().contains("authenticat") {
            ImapError::Auth(msg)
        } else {
            ImapError::Protocol(msg)
        }
    }
}
