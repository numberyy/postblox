//! CRUD for the `accounts` table.
//!
//! One row per configured mailbox: IMAP/SMTP endpoints, auth kind,
//! sync status, and the keyring `secret_ref` that points at the
//! [`crate::secrets::SecretStore`] entry. All functions take a shared
//! [`SqlitePool`] (the daemon owns the only pool — see CLAUDE.md's
//! "no module creates its own DB connection" rule) and return
//! [`crate::models::Account`] rows. Errors funnel through
//! [`DbError::Sqlx`].

use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::db::DbError;
use crate::models::{Account, AccountId, AuthKind, SyncStatus};

/// Input record for [`create`]: every column needed to insert a new
/// row into the `accounts` table.
#[derive(Debug, Clone, Deserialize)]
pub struct NewAccount {
    /// Primary email address used as the account's login identity.
    pub email: String,
    /// Optional display name (e.g. `"Alice Example"`).
    pub display_name: Option<String>,
    /// Authentication mechanism used for IMAP and SMTP.
    pub auth_kind: AuthKind,
    /// IMAP server hostname.
    pub imap_host: String,
    /// IMAP server port.
    pub imap_port: i64,
    /// Whether IMAP uses implicit TLS on connect.
    pub imap_use_tls: bool,
    /// SMTP submission server hostname.
    pub smtp_host: String,
    /// SMTP submission server port.
    pub smtp_port: i64,
    /// Whether SMTP uses implicit TLS on connect.
    pub smtp_use_tls: bool,
    /// Whether SMTP issues `STARTTLS` after connect.
    pub smtp_starttls: bool,
}

const SELECT: &str = "\
    id, email, display_name, auth_kind, imap_host, imap_port, imap_use_tls, \
    smtp_host, smtp_port, smtp_use_tls, smtp_starttls, secret_ref, last_synced_at, \
    sync_status, sync_error, created_at, updated_at";

/// Insert a new account row and return the persisted record.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the insert fails — most commonly a
/// `UNIQUE` violation on `email`, but also any other SQLite constraint
/// or IO error from the pool.
pub async fn create(pool: &SqlitePool, new: &NewAccount) -> Result<Account, DbError> {
    let id = AccountId::new();
    let q = format!(
        "INSERT INTO accounts \
         (id, email, display_name, auth_kind, imap_host, imap_port, imap_use_tls, \
          smtp_host, smtp_port, smtp_use_tls, smtp_starttls) \
         VALUES (?,?,?,?,?,?,?,?,?,?,?) RETURNING {SELECT}"
    );
    Ok(sqlx::query_as(&q)
        .bind(id)
        .bind(&new.email)
        .bind(&new.display_name)
        .bind(new.auth_kind)
        .bind(&new.imap_host)
        .bind(new.imap_port)
        .bind(new.imap_use_tls)
        .bind(&new.smtp_host)
        .bind(new.smtp_port)
        .bind(new.smtp_use_tls)
        .bind(new.smtp_starttls)
        .fetch_one(pool)
        .await?)
}

/// List all accounts, oldest first.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the query or row decode fails.
pub async fn list(pool: &SqlitePool) -> Result<Vec<Account>, DbError> {
    let q = format!("SELECT {SELECT} FROM accounts ORDER BY created_at");
    Ok(sqlx::query_as(&q).fetch_all(pool).await?)
}

/// Look up an account by id; `Ok(None)` if not present.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the query or row decode fails. A missing
/// row is reported as `Ok(None)`, not an error.
pub async fn get(pool: &SqlitePool, id: AccountId) -> Result<Option<Account>, DbError> {
    let q = format!("SELECT {SELECT} FROM accounts WHERE id = ?");
    Ok(sqlx::query_as(&q).bind(id).fetch_optional(pool).await?)
}

/// Look up an account by email; `Ok(None)` if not present.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the query or row decode fails. A missing
/// row is reported as `Ok(None)`, not an error.
pub async fn get_by_email(pool: &SqlitePool, email: &str) -> Result<Option<Account>, DbError> {
    let q = format!("SELECT {SELECT} FROM accounts WHERE email = ?");
    Ok(sqlx::query_as(&q).bind(email).fetch_optional(pool).await?)
}

/// Delete an account by id. Returns `true` if a row was removed.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the delete fails (FK or IO). A missing
/// row is reported as `Ok(false)`, not an error.
pub async fn delete(pool: &SqlitePool, id: AccountId) -> Result<bool, DbError> {
    let res = sqlx::query("DELETE FROM accounts WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

/// Update the `secret_ref` pointer for an account. Pass `None` to clear.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the update fails. A missing id is a
/// silent no-op (rows_affected = 0), not an error.
pub async fn set_secret_ref(
    pool: &SqlitePool,
    id: AccountId,
    secret_ref: Option<&str>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE accounts SET secret_ref = ?, \
         updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?",
    )
    .bind(secret_ref)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Update the per-account sync status, optional last error message,
/// and optional `last_synced_at` (preserves the existing value when
/// the parameter is `None`).
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the update fails. A missing id is a
/// silent no-op (rows_affected = 0), not an error.
pub async fn update_sync(
    pool: &SqlitePool,
    id: AccountId,
    status: SyncStatus,
    error: Option<&str>,
    last_synced_at: Option<DateTime<Utc>>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE accounts SET sync_status = ?, sync_error = ?, \
         last_synced_at = COALESCE(?, last_synced_at), \
         updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = ?",
    )
    .bind(status)
    .bind(error)
    .bind(last_synced_at)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(email: &str) -> NewAccount {
        NewAccount {
            email: email.into(),
            display_name: Some("Me".into()),
            auth_kind: AuthKind::Password,
            imap_host: "imap.example.com".into(),
            imap_port: 993,
            imap_use_tls: true,
            smtp_host: "smtp.example.com".into(),
            smtp_port: 465,
            smtp_use_tls: true,
            smtp_starttls: false,
        }
    }

    #[tokio::test]
    async fn test_create_get_round_trip() {
        let pool = crate::db::test_pool().await;
        let acc = create(&pool, &sample("a@x.com")).await.unwrap();
        assert_eq!(acc.email, "a@x.com");
        assert_eq!(acc.auth_kind, AuthKind::Password);
        assert_eq!(acc.imap_port, 993);
        assert!(acc.secret_ref.is_none());
        assert_eq!(acc.sync_status, SyncStatus::Idle);

        let got = get(&pool, acc.id).await.unwrap().unwrap();
        assert_eq!(got, acc);
    }

    #[tokio::test]
    async fn test_get_by_email_finds() {
        let pool = crate::db::test_pool().await;
        create(&pool, &sample("hit@x.com")).await.unwrap();
        let got = get_by_email(&pool, "hit@x.com").await.unwrap().unwrap();
        assert_eq!(got.email, "hit@x.com");
        assert!(get_by_email(&pool, "miss@x.com").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_unique_email_enforced() {
        let pool = crate::db::test_pool().await;
        create(&pool, &sample("dup@x.com")).await.unwrap();
        let err = create(&pool, &sample("dup@x.com")).await.unwrap_err();
        assert!(err.to_string().to_lowercase().contains("unique"));
    }

    #[tokio::test]
    async fn test_list_orders_by_created_at() {
        let pool = crate::db::test_pool().await;
        let a = create(&pool, &sample("first@x.com")).await.unwrap();
        // strftime resolution is ms so a tiny pause keeps determinism
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let b = create(&pool, &sample("second@x.com")).await.unwrap();
        let listed = list(&pool).await.unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, a.id);
        assert_eq!(listed[1].id, b.id);
    }

    #[tokio::test]
    async fn test_delete_returns_true_then_false() {
        let pool = crate::db::test_pool().await;
        let acc = create(&pool, &sample("d@x.com")).await.unwrap();
        assert!(delete(&pool, acc.id).await.unwrap());
        assert!(!delete(&pool, acc.id).await.unwrap());
        assert!(get(&pool, acc.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_set_secret_ref_updates_field() {
        let pool = crate::db::test_pool().await;
        let acc = create(&pool, &sample("s@x.com")).await.unwrap();
        set_secret_ref(&pool, acc.id, Some("kr/postblox/account/s@x.com/imap"))
            .await
            .unwrap();
        let got = get(&pool, acc.id).await.unwrap().unwrap();
        assert_eq!(
            got.secret_ref.as_deref(),
            Some("kr/postblox/account/s@x.com/imap")
        );

        set_secret_ref(&pool, acc.id, None).await.unwrap();
        let got = get(&pool, acc.id).await.unwrap().unwrap();
        assert!(got.secret_ref.is_none());
    }

    #[tokio::test]
    async fn test_update_sync_writes_status_and_error() {
        let pool = crate::db::test_pool().await;
        let acc = create(&pool, &sample("u@x.com")).await.unwrap();
        let when = Utc::now();
        update_sync(&pool, acc.id, SyncStatus::Syncing, None, Some(when))
            .await
            .unwrap();
        let got = get(&pool, acc.id).await.unwrap().unwrap();
        assert_eq!(got.sync_status, SyncStatus::Syncing);
        assert!(got.last_synced_at.is_some());
        assert!(got.sync_error.is_none());

        update_sync(&pool, acc.id, SyncStatus::Error, Some("oops"), None)
            .await
            .unwrap();
        let got = get(&pool, acc.id).await.unwrap().unwrap();
        assert_eq!(got.sync_status, SyncStatus::Error);
        assert_eq!(got.sync_error.as_deref(), Some("oops"));
        // last_synced_at preserved from previous write
        assert!(got.last_synced_at.is_some());
    }

    #[tokio::test]
    async fn test_update_sync_unknown_id_noop() {
        let pool = crate::db::test_pool().await;
        update_sync(&pool, AccountId::new(), SyncStatus::Idle, None, None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_get_unknown_returns_none() {
        let pool = crate::db::test_pool().await;
        assert!(get(&pool, AccountId::new()).await.unwrap().is_none());
    }
}
