//! Errors surfaced by the sync layer.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("imap: {0}")]
    Imap(#[from] crate::imap::ImapError),

    #[error("db: {0}")]
    Db(#[from] crate::db::DbError),

    #[error("attachment: {0}")]
    Attachment(#[from] crate::attachments::AttachmentError),

    #[error("parse: {0}")]
    Parse(#[from] crate::mail::error::MailError),

    #[error("unknown account")]
    UnknownAccount,

    #[error("unknown folder '{0}'")]
    UnknownFolder(String),

    #[error("missing credentials for account")]
    MissingCredentials,

    #[error("credential resolution failed: {0}")]
    Credential(String),
}
