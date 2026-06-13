//! Errors surfaced by the sync layer.

use thiserror::Error;

/// Error returned by the sync layer (reconciler, manager, worker).
#[derive(Debug, Error)]
pub enum SyncError {
    /// Underlying IMAP transport failure.
    #[error("imap: {0}")]
    Imap(#[from] crate::imap::ImapError),

    /// Local SQLite read/write failure.
    #[error("db: {0}")]
    Db(#[from] crate::db::DbError),

    /// Attachment persistence failed while ingesting a message.
    #[error("attachment: {0}")]
    Attachment(#[from] crate::attachments::AttachmentError),

    /// No account row matched the request.
    #[error("unknown account")]
    UnknownAccount,

    /// The named folder does not exist locally for the account.
    #[error("unknown folder '{0}'")]
    UnknownFolder(String),

    /// The account has no credentials registered in the secret store.
    #[error("missing credentials for account")]
    MissingCredentials,

    /// Credential resolution failed before the IMAP connection was attempted.
    #[error("credential resolution failed: {0}")]
    Credential(String),
}
