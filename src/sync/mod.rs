pub mod imap;

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("imap connection failed: {0}")]
    Connection(String),
    #[error("imap authentication failed")]
    Auth,
    #[error("imap protocol error: {0}")]
    Protocol(String),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

pub struct SyncResult {
    pub fetched: usize,
    pub stored: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)] // Idle used for DB reads, complete_sync sets it via SQL
pub enum SyncStatus {
    Idle,
    Syncing,
    Error,
}

impl std::fmt::Display for SyncStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for SyncStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "idle" => Ok(Self::Idle),
            "syncing" => Ok(Self::Syncing),
            "error" => Ok(Self::Error),
            other => Err(format!("unknown sync status: {other}")),
        }
    }
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

impl sqlx::Type<sqlx::Postgres> for SyncStatus {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as sqlx::Type<sqlx::Postgres>>::type_info()
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        <String as sqlx::Type<sqlx::Postgres>>::compatible(ty)
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for SyncStatus {
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        let s = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
        s.parse::<SyncStatus>()
            .map_err(|e| -> sqlx::error::BoxDynError { e.into() })
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for SyncStatus {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <&str as sqlx::Encode<sqlx::Postgres>>::encode(self.as_str(), buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_error_display_connection() {
        let err = SyncError::Connection("timeout".into());
        assert_eq!(err.to_string(), "imap connection failed: timeout");
    }

    #[test]
    fn test_sync_error_display_auth() {
        let err = SyncError::Auth;
        assert_eq!(err.to_string(), "imap authentication failed");
    }

    #[test]
    fn test_sync_error_display_protocol() {
        let err = SyncError::Protocol("bad response".into());
        assert_eq!(err.to_string(), "imap protocol error: bad response");
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
