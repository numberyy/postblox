pub mod imap;

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("IMAP connection failed: {0}")]
    Connection(String),
    #[error("IMAP authentication failed")]
    Auth,
    #[error("IMAP protocol error: {0}")]
    Protocol(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

pub struct SyncResult {
    pub fetched: usize,
    pub stored: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)] // Idle used for DB reads, complete_sync sets it via SQL
pub enum SyncStatus {
    Idle,
    Syncing,
    Error,
}

impl SyncStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Syncing => "syncing",
            Self::Error => "error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_error_display_connection() {
        let err = SyncError::Connection("timeout".into());
        assert_eq!(err.to_string(), "IMAP connection failed: timeout");
    }

    #[test]
    fn test_sync_error_display_auth() {
        let err = SyncError::Auth;
        assert_eq!(err.to_string(), "IMAP authentication failed");
    }

    #[test]
    fn test_sync_error_display_protocol() {
        let err = SyncError::Protocol("bad response".into());
        assert_eq!(err.to_string(), "IMAP protocol error: bad response");
    }

    #[test]
    fn test_sync_error_from_sqlx() {
        let sqlx_err = sqlx::Error::RowNotFound;
        let err: SyncError = sqlx_err.into();
        assert!(err.to_string().contains("database error"));
    }

    #[test]
    fn test_sync_result_fields() {
        let result = SyncResult {
            fetched: 10,
            stored: 8,
            skipped: 2,
        };
        assert_eq!(result.fetched, 10);
        assert_eq!(result.stored, 8);
        assert_eq!(result.skipped, 2);
    }
}
