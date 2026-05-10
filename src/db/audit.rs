//! Append-only audit log for user/agent actions.
//!
//! Records who (`actor` — `user`, `mcp:<tool>`, …) did what
//! (`action`) to which target, plus a free-form JSON `details`
//! payload. Pagination uses `(created_at DESC, rowid DESC)` so
//! same-millisecond inserts still produce a deterministic order.
//! Writes never block the caller's hot path — the daemon records
//! after the underlying op succeeds.

use serde_json::Value;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::db::DbError;
use crate::models::AuditEntry;

const COLS: &str = "id, actor, action, target, details, created_at";

#[derive(Debug, Clone)]
pub struct NewAuditEntry {
    pub actor: String,
    pub action: String,
    pub target: Option<String>,
    pub details: Value,
}

/// Insert an audit-log entry and return the persisted record.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the insert or the follow-up `SELECT` fails.
pub async fn record(pool: &SqlitePool, new: &NewAuditEntry) -> Result<AuditEntry, DbError> {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO audit_log (id, actor, action, target, details) VALUES (?,?,?,?,?)")
        .bind(id)
        .bind(&new.actor)
        .bind(&new.action)
        .bind(&new.target)
        .bind(&new.details)
        .execute(pool)
        .await?;
    Ok(
        sqlx::query_as::<_, AuditEntry>(&format!("SELECT {COLS} FROM audit_log WHERE id = ?"))
            .bind(id)
            .fetch_one(pool)
            .await?,
    )
}

/// List recent audit entries, newest first, with a stable rowid tie-break.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the query or row decode fails.
pub async fn list_recent(
    pool: &SqlitePool,
    limit: i64,
    offset: i64,
) -> Result<Vec<AuditEntry>, DbError> {
    // rowid is the strictly-monotonic insertion order; created_at can
    // collide on the same millisecond. Tie-break on rowid so pagination
    // is deterministic.
    Ok(sqlx::query_as::<_, AuditEntry>(&format!(
        "SELECT {COLS} FROM audit_log ORDER BY created_at DESC, rowid DESC LIMIT ? OFFSET ?"
    ))
    .bind(limit.clamp(1, 500))
    .bind(offset.max(0))
    .fetch_all(pool)
    .await?)
}

/// List recent audit entries for a single actor, newest first.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the query or row decode fails.
pub async fn list_by_actor(
    pool: &SqlitePool,
    actor: &str,
    limit: i64,
) -> Result<Vec<AuditEntry>, DbError> {
    Ok(sqlx::query_as::<_, AuditEntry>(&format!(
        "SELECT {COLS} FROM audit_log WHERE actor = ? \
         ORDER BY created_at DESC, rowid DESC LIMIT ?"
    ))
    .bind(actor)
    .bind(limit.clamp(1, 500))
    .fetch_all(pool)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_pool;
    use serde_json::json;

    fn entry(actor: &str, action: &str) -> NewAuditEntry {
        NewAuditEntry {
            actor: actor.into(),
            action: action.into(),
            target: Some("msg-123".into()),
            details: json!({"folder":"INBOX"}),
        }
    }

    #[tokio::test]
    async fn test_record_round_trip() {
        let pool = test_pool().await;
        let e = record(&pool, &entry("user", "archive")).await.unwrap();
        assert_eq!(e.action, "archive");
        assert_eq!(e.target.as_deref(), Some("msg-123"));
    }

    #[tokio::test]
    async fn test_list_recent_orders_desc_with_pagination() {
        let pool = test_pool().await;
        for i in 0..5 {
            record(&pool, &entry("user", &format!("a{i}")))
                .await
                .unwrap();
        }
        let page = list_recent(&pool, 2, 1).await.unwrap();
        assert_eq!(page.len(), 2);
        // Most recent is i=4; with offset=1 we expect i=3 then i=2.
        assert_eq!(page[0].action, "a3");
        assert_eq!(page[1].action, "a2");
    }

    #[tokio::test]
    async fn test_list_recent_negative_limit_clamps_to_one() {
        let pool = test_pool().await;
        for i in 0..3 {
            record(&pool, &entry("user", &format!("a{i}")))
                .await
                .unwrap();
        }
        let page = list_recent(&pool, -1, 0).await.unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].action, "a2");
    }

    #[tokio::test]
    async fn test_list_recent_oversized_limit_clamps_to_cap() {
        let pool = test_pool().await;
        for i in 0..501 {
            record(&pool, &entry("user", &format!("a{i}")))
                .await
                .unwrap();
        }
        let page = list_recent(&pool, 1_000, 0).await.unwrap();
        assert_eq!(page.len(), 500);
        assert_eq!(page[0].action, "a500");
        assert_eq!(page[499].action, "a1");
    }

    #[tokio::test]
    async fn test_list_recent_negative_offset_behaves_as_zero() {
        let pool = test_pool().await;
        for i in 0..3 {
            record(&pool, &entry("user", &format!("a{i}")))
                .await
                .unwrap();
        }
        let negative_offset = list_recent(&pool, 2, -10).await.unwrap();
        let zero_offset = list_recent(&pool, 2, 0).await.unwrap();
        let negative_actions: Vec<_> = negative_offset
            .iter()
            .map(|entry| entry.action.as_str())
            .collect();
        let zero_actions: Vec<_> = zero_offset
            .iter()
            .map(|entry| entry.action.as_str())
            .collect();
        assert_eq!(negative_actions, zero_actions);
    }

    #[tokio::test]
    async fn test_list_by_actor_filters() {
        let pool = test_pool().await;
        record(&pool, &entry("user", "send")).await.unwrap();
        record(&pool, &entry("mcp:send", "send")).await.unwrap();
        let user_only = list_by_actor(&pool, "user", 10).await.unwrap();
        assert_eq!(user_only.len(), 1);
        assert_eq!(user_only[0].actor, "user");
    }

    #[tokio::test]
    async fn test_list_by_actor_negative_limit_clamps_to_one() {
        let pool = test_pool().await;
        for i in 0..3 {
            record(&pool, &entry("user", &format!("a{i}")))
                .await
                .unwrap();
        }
        let user_only = list_by_actor(&pool, "user", -1).await.unwrap();
        assert_eq!(user_only.len(), 1);
        assert_eq!(user_only[0].action, "a2");
    }
}
