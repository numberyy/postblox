use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{CreateLinkedAccount, LinkedAccount};

pub async fn create(
    pool: &PgPool,
    account: &CreateLinkedAccount,
) -> Result<LinkedAccount, sqlx::Error> {
    let port = account.imap_port.unwrap_or(993);

    sqlx::query_as(
        "INSERT INTO linked_accounts (inbox_id, org_id, imap_host, imap_port, username, password) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING id, inbox_id, org_id, provider, imap_host, imap_port, username, \
         password, last_sync_at, sync_status, message_count, created_at",
    )
    .bind(account.inbox_id)
    .bind(account.org_id)
    .bind(&account.imap_host)
    .bind(port)
    .bind(&account.username)
    .bind(&account.password)
    .fetch_one(pool)
    .await
}

pub async fn get_by_id(pool: &PgPool, id: Uuid) -> Result<Option<LinkedAccount>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, inbox_id, org_id, provider, imap_host, imap_port, username, \
         password, last_sync_at, sync_status, message_count, created_at \
         FROM linked_accounts WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn list_by_org(pool: &PgPool, org_id: Uuid) -> Result<Vec<LinkedAccount>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, inbox_id, org_id, provider, imap_host, imap_port, username, \
         password, last_sync_at, sync_status, message_count, created_at \
         FROM linked_accounts WHERE org_id = $1 ORDER BY created_at DESC",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await
}

pub async fn delete(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM linked_accounts WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn set_sync_status(
    pool: &PgPool,
    id: Uuid,
    status: crate::sync::SyncStatus,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE linked_accounts SET sync_status = $2 WHERE id = $1")
        .bind(id)
        .bind(status)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn complete_sync(
    pool: &PgPool,
    id: Uuid,
    message_count: i32,
    last_sync_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE linked_accounts SET sync_status = 'idle', message_count = $2, last_sync_at = $3 \
         WHERE id = $1",
    )
    .bind(id)
    .bind(message_count)
    .bind(last_sync_at)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // requires running Postgres
    async fn test_linked_account_create_and_get() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "LA Org")
            .await
            .unwrap();
        let email = format!("la-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(&pool, org.id, &email, None, "linked")
            .await
            .unwrap();

        let input = CreateLinkedAccount {
            inbox_id: inbox.id,
            org_id: org.id,
            imap_host: "imap.example.com".into(),
            imap_port: Some(993),
            username: "user@example.com".into(),
            password: "enc_pass".into(),
        };
        let acct = create(&pool, &input).await.unwrap();
        assert_eq!(acct.inbox_id, inbox.id);
        assert_eq!(acct.org_id, org.id);
        assert_eq!(acct.imap_host, "imap.example.com");
        assert_eq!(acct.imap_port, 993);
        assert_eq!(acct.sync_status, crate::sync::SyncStatus::Idle);
        assert_eq!(acct.message_count, 0);
        assert!(acct.last_sync_at.is_none());

        let fetched = get_by_id(&pool, acct.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, acct.id);
    }

    #[tokio::test]
    #[ignore]
    async fn test_linked_account_defaults() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "LA Default Org")
            .await
            .unwrap();
        let email = format!("la-def-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(&pool, org.id, &email, None, "linked")
            .await
            .unwrap();

        let input = CreateLinkedAccount {
            inbox_id: inbox.id,
            org_id: org.id,
            imap_host: "imap.test.com".into(),
            imap_port: None,
            username: "user".into(),
            password: "pw".into(),
        };
        let acct = create(&pool, &input).await.unwrap();
        assert_eq!(acct.imap_port, 993);
        assert_eq!(acct.provider, "imap");
    }

    #[tokio::test]
    #[ignore]
    async fn test_linked_account_duplicate_inbox_fails() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "LA Dup Org")
            .await
            .unwrap();
        let email = format!("la-dup-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(&pool, org.id, &email, None, "linked")
            .await
            .unwrap();

        let input = CreateLinkedAccount {
            inbox_id: inbox.id,
            org_id: org.id,
            imap_host: "imap.one.com".into(),
            imap_port: None,
            username: "u".into(),
            password: "p".into(),
        };
        create(&pool, &input).await.unwrap();
        assert!(create(&pool, &input).await.is_err());
    }

    #[tokio::test]
    #[ignore]
    async fn test_linked_account_get_nonexistent_returns_none() {
        let pool = crate::db::test_pool().await;
        let result = get_by_id(&pool, Uuid::new_v4()).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_linked_account_list_by_org() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "LA List Org")
            .await
            .unwrap();

        for i in 0..2 {
            let email = format!("la-list-{i}-{}@example.com", Uuid::new_v4());
            let inbox = crate::db::inboxes::create(&pool, org.id, &email, None, "linked")
                .await
                .unwrap();
            let input = CreateLinkedAccount {
                inbox_id: inbox.id,
                org_id: org.id,
                imap_host: format!("imap{i}.example.com"),
                imap_port: None,
                username: format!("user{i}"),
                password: "pw".into(),
            };
            create(&pool, &input).await.unwrap();
        }

        let accounts = list_by_org(&pool, org.id).await.unwrap();
        assert_eq!(accounts.len(), 2);
    }

    #[tokio::test]
    #[ignore]
    async fn test_linked_account_list_by_org_empty() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "LA Empty Org")
            .await
            .unwrap();
        let accounts = list_by_org(&pool, org.id).await.unwrap();
        assert!(accounts.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_linked_account_list_by_org_isolation() {
        let pool = crate::db::test_pool().await;
        let org1 = crate::db::organizations::create(&pool, "LA Iso Org1")
            .await
            .unwrap();
        let org2 = crate::db::organizations::create(&pool, "LA Iso Org2")
            .await
            .unwrap();

        let email1 = format!("la-iso1-{}@example.com", Uuid::new_v4());
        let inbox1 = crate::db::inboxes::create(&pool, org1.id, &email1, None, "linked")
            .await
            .unwrap();
        let input1 = CreateLinkedAccount {
            inbox_id: inbox1.id,
            org_id: org1.id,
            imap_host: "imap.one.com".into(),
            imap_port: None,
            username: "u".into(),
            password: "p".into(),
        };
        create(&pool, &input1).await.unwrap();

        let accounts = list_by_org(&pool, org2.id).await.unwrap();
        assert!(accounts.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_linked_account_delete() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "LA Del Org")
            .await
            .unwrap();
        let email = format!("la-del-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(&pool, org.id, &email, None, "linked")
            .await
            .unwrap();

        let input = CreateLinkedAccount {
            inbox_id: inbox.id,
            org_id: org.id,
            imap_host: "imap.del.com".into(),
            imap_port: None,
            username: "u".into(),
            password: "p".into(),
        };
        let acct = create(&pool, &input).await.unwrap();
        assert!(delete(&pool, acct.id).await.unwrap());
        assert!(get_by_id(&pool, acct.id).await.unwrap().is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_linked_account_delete_nonexistent_returns_false() {
        let pool = crate::db::test_pool().await;
        assert!(!delete(&pool, Uuid::new_v4()).await.unwrap());
    }

    #[tokio::test]
    #[ignore]
    async fn test_linked_account_set_sync_status() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "LA Sync Org")
            .await
            .unwrap();
        let email = format!("la-sync-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(&pool, org.id, &email, None, "linked")
            .await
            .unwrap();

        let input = CreateLinkedAccount {
            inbox_id: inbox.id,
            org_id: org.id,
            imap_host: "imap.sync.com".into(),
            imap_port: None,
            username: "u".into(),
            password: "p".into(),
        };
        let acct = create(&pool, &input).await.unwrap();

        set_sync_status(&pool, acct.id, crate::sync::SyncStatus::Syncing)
            .await
            .unwrap();
        let fetched = get_by_id(&pool, acct.id).await.unwrap().unwrap();
        assert_eq!(fetched.sync_status, crate::sync::SyncStatus::Syncing);

        let now = Utc::now();
        complete_sync(&pool, acct.id, 15, now).await.unwrap();
        let fetched = get_by_id(&pool, acct.id).await.unwrap().unwrap();
        assert_eq!(fetched.sync_status, crate::sync::SyncStatus::Idle);
        assert_eq!(fetched.message_count, 15);
        assert!(fetched.last_sync_at.is_some());
    }
}
