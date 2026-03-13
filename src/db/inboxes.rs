use sqlx::PgPool;
use uuid::Uuid;

use crate::models::Inbox;

const SELECT_COLS: &str = "id, org_id, email, display_name, inbox_type, active, created_at";

pub async fn create(
    pool: &PgPool,
    org_id: Uuid,
    email: &str,
    display_name: Option<&str>,
    inbox_type: &str,
) -> Result<Inbox, sqlx::Error> {
    sqlx::query_as(&format!(
        "INSERT INTO inboxes (org_id, email, display_name, inbox_type) \
         VALUES ($1, $2, $3, $4) RETURNING {SELECT_COLS}"
    ))
    .bind(org_id)
    .bind(email)
    .bind(display_name)
    .bind(inbox_type)
    .fetch_one(pool)
    .await
}

pub async fn get_by_id(pool: &PgPool, id: Uuid) -> Result<Option<Inbox>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {SELECT_COLS} FROM inboxes WHERE id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn get_by_email(pool: &PgPool, email: &str) -> Result<Option<Inbox>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {SELECT_COLS} FROM inboxes WHERE email = $1"
    ))
    .bind(email)
    .fetch_optional(pool)
    .await
}

pub async fn list_by_org(pool: &PgPool, org_id: Uuid) -> Result<Vec<Inbox>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {SELECT_COLS} FROM inboxes WHERE org_id = $1 ORDER BY created_at"
    ))
    .bind(org_id)
    .fetch_all(pool)
    .await
}

pub async fn set_active(pool: &PgPool, id: Uuid, active: bool) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("UPDATE inboxes SET active = $2 WHERE id = $1")
        .bind(id)
        .bind(active)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn delete(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM inboxes WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn test_inbox_create_and_get() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Inbox Org")
            .await
            .unwrap();
        let email = format!("bot-{}@example.com", Uuid::new_v4());

        let inbox = create(&pool, org.id, &email, Some("Bot"), "native")
            .await
            .unwrap();
        assert_eq!(inbox.email, email);
        assert_eq!(inbox.display_name.as_deref(), Some("Bot"));
        assert_eq!(inbox.inbox_type, "native");

        let fetched = get_by_id(&pool, inbox.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, inbox.id);
    }

    #[tokio::test]
    #[ignore]
    async fn test_inbox_duplicate_email_fails() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Dup Org")
            .await
            .unwrap();
        let email = format!("dup-{}@example.com", Uuid::new_v4());

        create(&pool, org.id, &email, None, "native").await.unwrap();
        let err = create(&pool, org.id, &email, None, "native").await;
        assert!(err.is_err());
    }

    #[tokio::test]
    #[ignore]
    async fn test_inbox_get_by_email() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Email Lookup Org")
            .await
            .unwrap();
        let email = format!("lookup-{}@example.com", Uuid::new_v4());

        let inbox = create(&pool, org.id, &email, None, "native").await.unwrap();
        let found = get_by_email(&pool, &email).await.unwrap().unwrap();
        assert_eq!(found.id, inbox.id);
    }

    #[tokio::test]
    #[ignore]
    async fn test_inbox_get_by_email_nonexistent() {
        let pool = crate::db::test_pool().await;
        let result = get_by_email(&pool, "nope@nowhere.com").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_inbox_list_by_org() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "List Inbox Org")
            .await
            .unwrap();

        let e1 = format!("a-{}@example.com", Uuid::new_v4());
        let e2 = format!("b-{}@example.com", Uuid::new_v4());
        create(&pool, org.id, &e1, None, "native").await.unwrap();
        create(&pool, org.id, &e2, None, "relay").await.unwrap();

        let inboxes = list_by_org(&pool, org.id).await.unwrap();
        assert_eq!(inboxes.len(), 2);
    }

    #[tokio::test]
    #[ignore]
    async fn test_inbox_list_by_org_empty() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Empty Inbox Org")
            .await
            .unwrap();
        let inboxes = list_by_org(&pool, org.id).await.unwrap();
        assert!(inboxes.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_inbox_delete() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Del Org")
            .await
            .unwrap();
        let email = format!("del-{}@example.com", Uuid::new_v4());
        let inbox = create(&pool, org.id, &email, None, "native").await.unwrap();

        assert!(delete(&pool, inbox.id).await.unwrap());
        assert!(get_by_id(&pool, inbox.id).await.unwrap().is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_inbox_delete_nonexistent_returns_false() {
        let pool = crate::db::test_pool().await;
        assert!(!delete(&pool, Uuid::new_v4()).await.unwrap());
    }
}
