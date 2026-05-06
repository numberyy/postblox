use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::models::{Account, AuthKind, SyncStatus};

#[derive(Debug, Clone, Deserialize)]
pub struct NewAccount {
    pub email: String,
    pub display_name: Option<String>,
    pub auth_kind: AuthKind,
    pub imap_host: String,
    pub imap_port: i64,
    pub imap_use_tls: bool,
    pub smtp_host: String,
    pub smtp_port: i64,
    pub smtp_use_tls: bool,
    pub smtp_starttls: bool,
}

const SELECT: &str = "\
    id, email, display_name, auth_kind, imap_host, imap_port, imap_use_tls, \
    smtp_host, smtp_port, smtp_use_tls, smtp_starttls, secret_ref, last_synced_at, \
    sync_status, sync_error, created_at, updated_at";

pub async fn create(pool: &SqlitePool, new: &NewAccount) -> Result<Account, sqlx::Error> {
    let id = Uuid::new_v4();
    let q = format!(
        "INSERT INTO accounts \
         (id, email, display_name, auth_kind, imap_host, imap_port, imap_use_tls, \
          smtp_host, smtp_port, smtp_use_tls, smtp_starttls) \
         VALUES (?,?,?,?,?,?,?,?,?,?,?) RETURNING {SELECT}"
    );
    sqlx::query_as(&q)
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
        .await
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<Account>, sqlx::Error> {
    let q = format!("SELECT {SELECT} FROM accounts ORDER BY created_at");
    sqlx::query_as(&q).fetch_all(pool).await
}

pub async fn get(pool: &SqlitePool, id: Uuid) -> Result<Option<Account>, sqlx::Error> {
    let q = format!("SELECT {SELECT} FROM accounts WHERE id = ?");
    sqlx::query_as(&q).bind(id).fetch_optional(pool).await
}

pub async fn get_by_email(pool: &SqlitePool, email: &str) -> Result<Option<Account>, sqlx::Error> {
    let q = format!("SELECT {SELECT} FROM accounts WHERE email = ?");
    sqlx::query_as(&q).bind(email).fetch_optional(pool).await
}

pub async fn delete(pool: &SqlitePool, id: Uuid) -> Result<bool, sqlx::Error> {
    let res = sqlx::query("DELETE FROM accounts WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

pub async fn set_secret_ref(
    pool: &SqlitePool,
    id: Uuid,
    secret_ref: Option<&str>,
) -> Result<(), sqlx::Error> {
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

pub async fn update_sync(
    pool: &SqlitePool,
    id: Uuid,
    status: SyncStatus,
    error: Option<&str>,
    last_synced_at: Option<DateTime<Utc>>,
) -> Result<(), sqlx::Error> {
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
        update_sync(&pool, Uuid::new_v4(), SyncStatus::Idle, None, None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_get_unknown_returns_none() {
        let pool = crate::db::test_pool().await;
        assert!(get(&pool, Uuid::new_v4()).await.unwrap().is_none());
    }
}
