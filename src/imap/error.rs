//! Errors for the IMAP transport.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ImapError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("tls: {0}")]
    Tls(String),

    #[error("imap protocol: {0}")]
    Protocol(String),

    #[error("auth failed: {0}")]
    Auth(String),

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
