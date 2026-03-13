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

pub async fn delete(pool: &PgPool, id: Uuid, org_id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM api_keys WHERE id = $1 AND org_id = $2")
        .bind(id)
        .bind(org_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
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

    fn unique_prefix() -> String {
        format!("pb_{}", &Uuid::new_v4().to_string()[..8])
    }

    #[tokio::test]
    #[ignore]
    async fn test_api_key_create_and_find_by_prefix() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Key Test Org")
            .await
            .unwrap();

        let prefix = unique_prefix();
        let key = create(&pool, org.id, "hash_abc", &prefix, Some("my key"))
            .await
            .unwrap();
        assert_eq!(key.prefix, prefix);
        assert_eq!(key.name.as_deref(), Some("my key"));
        assert!(key.last_used_at.is_none());

        let found = find_by_prefix(&pool, &prefix).await.unwrap().unwrap();
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

        create(&pool, org.id, "h1", &unique_prefix(), None)
            .await
            .unwrap();
        create(&pool, org.id, "h2", &unique_prefix(), None)
            .await
            .unwrap();

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
    async fn test_api_key_delete_own_key() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Delete Key Org")
            .await
            .unwrap();
        let prefix = unique_prefix();
        let key = create(&pool, org.id, "h", &prefix, None).await.unwrap();
        assert!(delete(&pool, key.id, org.id).await.unwrap());
        assert!(find_by_prefix(&pool, &prefix).await.unwrap().is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_api_key_delete_wrong_org_returns_false() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Del Wrong Org")
            .await
            .unwrap();
        let prefix = unique_prefix();
        let key = create(&pool, org.id, "h", &prefix, None).await.unwrap();
        let other_org = crate::db::organizations::create(&pool, "Del Other Org")
            .await
            .unwrap();
        assert!(!delete(&pool, key.id, other_org.id).await.unwrap());
        assert!(find_by_prefix(&pool, &prefix).await.unwrap().is_some());
    }

    #[tokio::test]
    #[ignore]
    async fn test_api_key_delete_nonexistent_returns_false() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Del Nonexist Org")
            .await
            .unwrap();
        assert!(!delete(&pool, Uuid::new_v4(), org.id).await.unwrap());
    }

    #[tokio::test]
    #[ignore]
    async fn test_api_key_touch_last_used() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Touch Org")
            .await
            .unwrap();
        let prefix = unique_prefix();
        let key = create(&pool, org.id, "h", &prefix, None).await.unwrap();
        assert!(key.last_used_at.is_none());

        touch_last_used(&pool, key.id).await.unwrap();

        let updated = find_by_prefix(&pool, &prefix).await.unwrap().unwrap();
        assert!(updated.last_used_at.is_some());
    }
}
