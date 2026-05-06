//! Errors surfaced by the sync layer.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("imap: {0}")]
    Imap(#[from] crate::imap::ImapError),

    #[error("db: {0}")]
    Db(#[from] sqlx::Error),

    #[error("parse: {0}")]
    Parse(#[from] crate::mail::error::MailError),

    #[error("unknown account")]
    UnknownAccount,

    #[error("unknown folder '{0}'")]
    UnknownFolder(String),

    #[error("missing credentials for account")]
    MissingCredentials,
}
