use sqlx::PgPool;
use uuid::Uuid;

use crate::models::Organization;

pub async fn create(pool: &PgPool, name: &str) -> Result<Organization, sqlx::Error> {
    sqlx::query_as("INSERT INTO organizations (name) VALUES ($1) RETURNING id, name, created_at")
        .bind(name)
        .fetch_one(pool)
        .await
}

#[allow(dead_code)]
pub async fn get_by_id(pool: &PgPool, id: Uuid) -> Result<Option<Organization>, sqlx::Error> {
    sqlx::query_as("SELECT id, name, created_at FROM organizations WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
}

#[allow(dead_code)]
pub async fn list(pool: &PgPool) -> Result<Vec<Organization>, sqlx::Error> {
    sqlx::query_as("SELECT id, name, created_at FROM organizations ORDER BY created_at")
        .fetch_all(pool)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn test_organization_create_and_get() {
        let pool = crate::db::test_pool().await;
        let org = create(&pool, "Test Org").await.unwrap();
        assert_eq!(org.name, "Test Org");
        assert!(!org.id.is_nil());

        let fetched = get_by_id(&pool, org.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, org.id);
        assert_eq!(fetched.name, org.name);
    }

    #[tokio::test]
    #[ignore]
    async fn test_organization_get_nonexistent_returns_none() {
        let pool = crate::db::test_pool().await;
        let result = get_by_id(&pool, Uuid::new_v4()).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_organization_list_includes_created() {
        let pool = crate::db::test_pool().await;
        let org = create(&pool, "List Test Org").await.unwrap();
        let all = list(&pool).await.unwrap();
        assert!(all.iter().any(|o| o.id == org.id));
    }

    #[tokio::test]
    #[ignore]
    async fn test_organization_create_empty_name() {
        let pool = crate::db::test_pool().await;
        let org = create(&pool, "").await.unwrap();
        assert_eq!(org.name, "");
    }
}
