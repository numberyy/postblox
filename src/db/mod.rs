//! SQLite access layer. One module per entity; each module owns its
//! SQL and returns domain types from `crate::models`.

use std::path::Path;
use std::str::FromStr;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;

pub mod accounts;
pub mod attachments;
pub mod audit;
pub mod draft_attachments;
pub mod drafts;
pub mod folders;
pub mod mcp;
pub mod messages;
pub mod search;
pub mod threads;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlite error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

/// Open the database at `path`, creating it if missing, and run pending
/// migrations. Enables WAL + foreign keys.
pub async fn connect(path: &Path) -> Result<SqlitePool, DbError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                DbError::Sqlx(sqlx::Error::Configuration(
                    format!("create db parent dir {}: {e}", parent.display()).into(),
                ))
            })?;
        }
    }

    let url = format!("sqlite://{}?mode=rwc", path.display());
    let opts = SqliteConnectOptions::from_str(&url)?
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(std::time::Duration::from_secs(5));

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .acquire_timeout(std::time::Duration::from_secs(3))
        .connect_with(opts)
        .await?;

    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

/// In-memory pool for tests. Migrations applied. Single connection so the
/// schema is shared across awaits.
#[cfg(test)]
pub(crate) async fn test_pool() -> SqlitePool {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .expect("connect in-memory sqlite");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run migrations");
    pool
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_connect_creates_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("postblox.db");
        let pool = connect(&path).await.unwrap();

        let tables: Vec<(String,)> =
            sqlx::query_as("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
                .fetch_all(&pool)
                .await
                .unwrap();
        let names: Vec<_> = tables.iter().map(|r| r.0.as_str()).collect();

        for expected in [
            "accounts",
            "attachments",
            "audit_log",
            "draft_attachments",
            "drafts",
            "folders",
            "mcp_approvals",
            "mcp_gates",
            "messages",
            "messages_fts",
            "threads",
        ] {
            assert!(names.contains(&expected), "missing table {expected}");
        }
    }

    #[tokio::test]
    async fn test_connect_creates_parent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/sub/postblox.db");
        let pool = connect(&path).await.unwrap();
        sqlx::query("SELECT 1").execute(&pool).await.unwrap();
    }

    #[tokio::test]
    async fn test_in_memory_test_pool_works() {
        let pool = test_pool().await;
        let count: (i64,) = sqlx::query_as("SELECT count(*) FROM accounts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count.0, 0);
    }
}
