//! CRUD for the `threads` table.
//!
//! A thread groups messages that share an `external_id` per account
//! (or are stitched together by [`crate::mail::threading`]). Each row
//! tracks `last_message_at` and `message_count` so list views can
//! sort and badge without a join. `(account_id, external_id)` is
//! unique; inserts collide on that constraint. All access is via the
//! daemon's shared [`SqlitePool`].

use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::db::DbError;
use crate::models::Thread;

const SELECT: &str = "\
    id, account_id, external_id, subject, last_message_at, message_count, created_at";

/// Insert a thread row and return the persisted record.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the insert fails — typically a `UNIQUE`
/// violation on `(account_id, external_id)` or a FK violation when
/// `account_id` is unknown, but also any other SQLite error.
pub async fn create(
    pool: &SqlitePool,
    account_id: Uuid,
    external_id: Option<&str>,
    subject: Option<&str>,
) -> Result<Thread, DbError> {
    let id = Uuid::new_v4();
    let q = format!(
        "INSERT INTO threads (id, account_id, external_id, subject) \
         VALUES (?,?,?,?) RETURNING {SELECT}"
    );
    Ok(sqlx::query_as(&q)
        .bind(id)
        .bind(account_id)
        .bind(external_id)
        .bind(subject)
        .fetch_one(pool)
        .await?)
}

/// Look up a thread by id; `Ok(None)` if missing.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the query or row decode fails. A missing
/// row is reported as `Ok(None)`, not an error.
pub async fn get(pool: &SqlitePool, id: Uuid) -> Result<Option<Thread>, DbError> {
    let q = format!("SELECT {SELECT} FROM threads WHERE id = ?");
    Ok(sqlx::query_as(&q).bind(id).fetch_optional(pool).await?)
}

/// Look up a thread by `(account_id, external_id)`; `Ok(None)` if missing.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the query or row decode fails. A missing
/// row is reported as `Ok(None)`, not an error.
pub async fn get_by_external_id(
    pool: &SqlitePool,
    account_id: Uuid,
    external_id: &str,
) -> Result<Option<Thread>, DbError> {
    let q = format!("SELECT {SELECT} FROM threads WHERE account_id = ? AND external_id = ?");
    Ok(sqlx::query_as(&q)
        .bind(account_id)
        .bind(external_id)
        .fetch_optional(pool)
        .await?)
}

/// Recent threads for an account, newest first. Used by sidebar/list views
/// and by the in-Rust thread-matcher.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the query or row decode fails.
pub async fn list_recent(
    pool: &SqlitePool,
    account_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<Thread>, DbError> {
    let q = format!(
        "SELECT {SELECT} FROM threads WHERE account_id = ? \
         ORDER BY last_message_at DESC NULLS LAST, created_at DESC \
         LIMIT ? OFFSET ?"
    );
    Ok(sqlx::query_as(&q)
        .bind(account_id)
        .bind(limit.clamp(1, 500))
        .bind(offset.max(0))
        .fetch_all(pool)
        .await?)
}

/// Recompute the thread's `message_count` and `last_message_at` from its
/// messages. Cheap because messages are indexed by thread_id.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the update fails. A missing id is a
/// silent no-op (rows_affected = 0), not an error.
pub async fn refresh_aggregates(pool: &SqlitePool, id: Uuid) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE threads SET \
         message_count = (SELECT count(*) FROM messages WHERE thread_id = ?), \
         last_message_at = (SELECT max(internal_date) FROM messages WHERE thread_id = ?) \
         WHERE id = ?",
    )
    .bind(id)
    .bind(id)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Advance `last_message_at` to `when` only if `when` is strictly newer
/// than the current value (or the column is `NULL`). Older timestamps
/// are ignored, so retries during catch-up sync are safe.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the update fails. A missing id, or a
/// `when` that is not strictly newer, is a silent no-op (rows_affected
/// = 0), not an error.
pub async fn touch_last_message_at(
    pool: &SqlitePool,
    id: Uuid,
    when: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE threads SET last_message_at = ? \
         WHERE id = ? AND (last_message_at IS NULL OR last_message_at < ?)",
    )
    .bind(when)
    .bind(id)
    .bind(when)
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete a thread by id. Returns `true` if a row was removed.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the delete fails (FK or IO). A missing
/// row is reported as `Ok(false)`, not an error.
pub async fn delete(pool: &SqlitePool, id: Uuid) -> Result<bool, DbError> {
    let r = sqlx::query("DELETE FROM threads WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn account(pool: &SqlitePool) -> Uuid {
        crate::db::accounts::create(
            pool,
            &crate::db::accounts::NewAccount {
                email: format!("u-{}@x.com", Uuid::new_v4()),
                display_name: None,
                auth_kind: crate::models::AuthKind::Password,
                imap_host: "i".into(),
                imap_port: 993,
                imap_use_tls: true,
                smtp_host: "s".into(),
                smtp_port: 465,
                smtp_use_tls: true,
                smtp_starttls: false,
            },
        )
        .await
        .unwrap()
        .id
    }

    #[tokio::test]
    async fn test_create_and_get() {
        let pool = crate::db::test_pool().await;
        let a = account(&pool).await;
        let t = create(&pool, a, Some("ext-1"), Some("Hi")).await.unwrap();
        assert_eq!(t.account_id, a);
        assert_eq!(t.message_count, 0);
        let got = get(&pool, t.id).await.unwrap().unwrap();
        assert_eq!(got, t);
    }

    #[tokio::test]
    async fn test_get_by_external_id() {
        let pool = crate::db::test_pool().await;
        let a = account(&pool).await;
        let t = create(&pool, a, Some("ext-7"), None).await.unwrap();
        let got = get_by_external_id(&pool, a, "ext-7")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.id, t.id);
        assert!(get_by_external_id(&pool, a, "missing")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_unique_external_id_per_account() {
        let pool = crate::db::test_pool().await;
        let a = account(&pool).await;
        create(&pool, a, Some("dup"), None).await.unwrap();
        let err = create(&pool, a, Some("dup"), None).await.unwrap_err();
        assert!(err.to_string().to_lowercase().contains("unique"));
    }

    #[tokio::test]
    async fn test_external_id_distinct_per_account() {
        let pool = crate::db::test_pool().await;
        let a = account(&pool).await;
        let b = account(&pool).await;
        create(&pool, a, Some("ext"), None).await.unwrap();
        create(&pool, b, Some("ext"), None).await.unwrap(); // ok, different account
    }

    #[tokio::test]
    async fn test_touch_last_message_at_only_advances() {
        let pool = crate::db::test_pool().await;
        let a = account(&pool).await;
        let t = create(&pool, a, None, None).await.unwrap();
        let now = Utc::now();
        touch_last_message_at(&pool, t.id, now).await.unwrap();
        let got = get(&pool, t.id).await.unwrap().unwrap();
        assert_eq!(got.last_message_at.unwrap().timestamp(), now.timestamp());

        // Earlier timestamp must not overwrite.
        touch_last_message_at(&pool, t.id, now - chrono::Duration::days(1))
            .await
            .unwrap();
        let got = get(&pool, t.id).await.unwrap().unwrap();
        assert_eq!(got.last_message_at.unwrap().timestamp(), now.timestamp());
    }

    #[tokio::test]
    async fn test_list_recent_clamps_limit_and_offset() {
        let pool = crate::db::test_pool().await;
        let a = account(&pool).await;
        for i in 0..3 {
            create(&pool, a, Some(&format!("e{i}")), None)
                .await
                .unwrap();
        }
        let listed = list_recent(&pool, a, -1, -5).await.unwrap();
        assert!(!listed.is_empty()); // -1 clamped to 1
        let none = list_recent(&pool, a, 10, 100).await.unwrap();
        assert!(none.is_empty());
    }
}
