use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{Domain, DomainStatus};

pub async fn create(pool: &PgPool, org_id: Uuid, name: &str) -> Result<Domain, sqlx::Error> {
    sqlx::query_as(
        "INSERT INTO domains (org_id, name) \
         VALUES ($1, $2) \
         RETURNING id, org_id, name, status, stalwart_principal_id, verified_at, created_at",
    )
    .bind(org_id)
    .bind(name)
    .fetch_one(pool)
    .await
}

pub async fn get_by_id(pool: &PgPool, id: Uuid) -> Result<Option<Domain>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, org_id, name, status, stalwart_principal_id, verified_at, created_at \
         FROM domains WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn list_by_org(pool: &PgPool, org_id: Uuid) -> Result<Vec<Domain>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, org_id, name, status, stalwart_principal_id, verified_at, created_at \
         FROM domains WHERE org_id = $1 ORDER BY name",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await
}

pub async fn update_status(
    pool: &PgPool,
    id: Uuid,
    status: DomainStatus,
    stalwart_principal_id: Option<&str>,
) -> Result<Option<Domain>, sqlx::Error> {
    sqlx::query_as(
        "UPDATE domains SET status = $2, stalwart_principal_id = $3 \
         WHERE id = $1 \
         RETURNING id, org_id, name, status, stalwart_principal_id, verified_at, created_at",
    )
    .bind(id)
    .bind(status)
    .bind(stalwart_principal_id)
    .fetch_optional(pool)
    .await
}

pub async fn set_verified(pool: &PgPool, id: Uuid) -> Result<Option<Domain>, sqlx::Error> {
    sqlx::query_as(
        "UPDATE domains SET status = 'verified', verified_at = now() \
         WHERE id = $1 \
         RETURNING id, org_id, name, status, stalwart_principal_id, verified_at, created_at",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn delete(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM domains WHERE id = $1")
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
    async fn test_domain_create_and_get() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Domain Org")
            .await
            .unwrap();
        let name = format!("{}.example.com", Uuid::new_v4());

        let domain = create(&pool, org.id, &name).await.unwrap();
        assert_eq!(domain.name, name);
        assert_eq!(domain.status, DomainStatus::Pending);
        assert!(domain.stalwart_principal_id.is_none());
        assert!(domain.verified_at.is_none());

        let fetched = get_by_id(&pool, domain.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, domain.id);
        assert_eq!(fetched.org_id, org.id);
    }

    #[tokio::test]
    #[ignore]
    async fn test_domain_duplicate_name_fails() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Dup Domain Org")
            .await
            .unwrap();
        let name = format!("{}.example.com", Uuid::new_v4());

        create(&pool, org.id, &name).await.unwrap();
        let err = create(&pool, org.id, &name).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    #[ignore]
    async fn test_domain_get_nonexistent_returns_none() {
        let pool = crate::db::test_pool().await;
        let result = get_by_id(&pool, Uuid::new_v4()).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_domain_list_by_org_ordered_by_name() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "List Domain Org")
            .await
            .unwrap();

        create(&pool, org.id, &format!("b-{}.example.com", Uuid::new_v4()))
            .await
            .unwrap();
        create(&pool, org.id, &format!("a-{}.example.com", Uuid::new_v4()))
            .await
            .unwrap();

        let domains = list_by_org(&pool, org.id).await.unwrap();
        assert_eq!(domains.len(), 2);
        assert!(domains[0].name < domains[1].name);
    }

    #[tokio::test]
    #[ignore]
    async fn test_domain_list_by_org_empty() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Empty Domain Org")
            .await
            .unwrap();
        let domains = list_by_org(&pool, org.id).await.unwrap();
        assert!(domains.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_domain_list_by_org_isolation() {
        let pool = crate::db::test_pool().await;
        let org1 = crate::db::organizations::create(&pool, "Domain Org 1")
            .await
            .unwrap();
        let org2 = crate::db::organizations::create(&pool, "Domain Org 2")
            .await
            .unwrap();

        create(&pool, org1.id, &format!("{}.example.com", Uuid::new_v4()))
            .await
            .unwrap();
        create(&pool, org2.id, &format!("{}.example.com", Uuid::new_v4()))
            .await
            .unwrap();

        let domains1 = list_by_org(&pool, org1.id).await.unwrap();
        assert_eq!(domains1.len(), 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_domain_update_status_with_principal_id() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Update Domain Org")
            .await
            .unwrap();
        let name = format!("{}.example.com", Uuid::new_v4());
        let domain = create(&pool, org.id, &name).await.unwrap();

        let updated = update_status(
            &pool,
            domain.id,
            DomainStatus::Verified,
            Some("principal-123"),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(updated.status, DomainStatus::Verified);
        assert_eq!(
            updated.stalwart_principal_id.as_deref(),
            Some("principal-123")
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_domain_update_status_nonexistent_returns_none() {
        let pool = crate::db::test_pool().await;
        let result = update_status(&pool, Uuid::new_v4(), DomainStatus::Verified, None)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_domain_set_verified() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Verify Domain Org")
            .await
            .unwrap();
        let name = format!("{}.example.com", Uuid::new_v4());
        let domain = create(&pool, org.id, &name).await.unwrap();
        assert_eq!(domain.status, DomainStatus::Pending);

        let verified = set_verified(&pool, domain.id).await.unwrap().unwrap();
        assert_eq!(verified.status, DomainStatus::Verified);
        assert!(verified.verified_at.is_some());
    }

    #[tokio::test]
    #[ignore]
    async fn test_domain_set_verified_nonexistent_returns_none() {
        let pool = crate::db::test_pool().await;
        let result = set_verified(&pool, Uuid::new_v4()).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_domain_delete() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Del Domain Org")
            .await
            .unwrap();
        let name = format!("{}.example.com", Uuid::new_v4());
        let domain = create(&pool, org.id, &name).await.unwrap();

        assert!(delete(&pool, domain.id).await.unwrap());
        assert!(get_by_id(&pool, domain.id).await.unwrap().is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_domain_delete_nonexistent_returns_false() {
        let pool = crate::db::test_pool().await;
        assert!(!delete(&pool, Uuid::new_v4()).await.unwrap());
    }
}
