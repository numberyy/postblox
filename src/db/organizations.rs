use sqlx::PgPool;

use crate::models::Organization;

pub async fn count(pool: &PgPool) -> Result<i64, sqlx::Error> {
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM organizations")
        .fetch_one(pool)
        .await?;
    Ok(count)
}

pub async fn create(pool: &PgPool, name: &str) -> Result<Organization, sqlx::Error> {
    sqlx::query_as("INSERT INTO organizations (name) VALUES ($1) RETURNING id, name, created_at")
        .bind(name)
        .fetch_one(pool)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn test_organization_create() {
        let pool = crate::db::test_pool().await;
        let org = create(&pool, "Test Org").await.unwrap();
        assert_eq!(org.name, "Test Org");
        assert!(!org.id.is_nil());
    }

    #[tokio::test]
    #[ignore]
    async fn test_organization_create_empty_name() {
        let pool = crate::db::test_pool().await;
        let org = create(&pool, "").await.unwrap();
        assert_eq!(org.name, "");
    }
}
