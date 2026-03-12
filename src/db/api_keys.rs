use sqlx::PgPool;
use uuid::Uuid;

use crate::models::ApiKey;

pub async fn create(
    pool: &PgPool,
    org_id: Uuid,
    key_hash: &str,
    prefix: &str,
    name: Option<&str>,
) -> Result<ApiKey, sqlx::Error> {
    sqlx::query_as(
        "INSERT INTO api_keys (org_id, key_hash, prefix, name) \
         VALUES ($1, $2, $3, $4) \
         RETURNING id, org_id, key_hash, prefix, name, created_at, last_used_at",
    )
    .bind(org_id)
    .bind(key_hash)
    .bind(prefix)
    .bind(name)
    .fetch_one(pool)
    .await
}

pub async fn find_by_prefix(pool: &PgPool, prefix: &str) -> Result<Option<ApiKey>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, org_id, key_hash, prefix, name, created_at, last_used_at \
         FROM api_keys WHERE prefix = $1",
    )
    .bind(prefix)
    .fetch_optional(pool)
    .await
}

pub async fn list_by_org(pool: &PgPool, org_id: Uuid) -> Result<Vec<ApiKey>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, org_id, key_hash, prefix, name, created_at, last_used_at \
         FROM api_keys WHERE org_id = $1 ORDER BY created_at",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await
}

pub async fn touch_last_used(pool: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE api_keys SET last_used_at = now() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Integration tests — require DATABASE_URL with migrations applied

    #[tokio::test]
    #[ignore]
    async fn test_api_key_create_and_find_by_prefix() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Key Test Org")
            .await
            .unwrap();

        let key = create(&pool, org.id, "hash_abc", "pb_test1", Some("my key"))
            .await
            .unwrap();
        assert_eq!(key.prefix, "pb_test1");
        assert_eq!(key.name.as_deref(), Some("my key"));
        assert!(key.last_used_at.is_none());

        let found = find_by_prefix(&pool, "pb_test1").await.unwrap().unwrap();
        assert_eq!(found.id, key.id);
    }

    #[tokio::test]
    #[ignore]
    async fn test_api_key_find_by_wrong_prefix_returns_none() {
        let pool = crate::db::test_pool().await;
        let result = find_by_prefix(&pool, "pb_nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_api_key_list_by_org() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Key List Org")
            .await
            .unwrap();

        create(&pool, org.id, "h1", "pb_list1", None).await.unwrap();
        create(&pool, org.id, "h2", "pb_list2", None).await.unwrap();

        let keys = list_by_org(&pool, org.id).await.unwrap();
        assert_eq!(keys.len(), 2);
    }

    #[tokio::test]
    #[ignore]
    async fn test_api_key_list_by_org_empty() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Empty Key Org")
            .await
            .unwrap();
        let keys = list_by_org(&pool, org.id).await.unwrap();
        assert!(keys.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_api_key_touch_last_used() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Touch Org")
            .await
            .unwrap();
        let key = create(&pool, org.id, "h", "pb_touch", None).await.unwrap();
        assert!(key.last_used_at.is_none());

        touch_last_used(&pool, key.id).await.unwrap();

        let updated = find_by_prefix(&pool, "pb_touch").await.unwrap().unwrap();
        assert!(updated.last_used_at.is_some());
    }
}
