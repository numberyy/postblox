use sqlx::PgPool;
use thiserror::Error;
use uuid::Uuid;

use crate::models::{OrgMember, Role};

#[derive(Debug, Error)]
pub enum MemberError {
    #[error("cannot remove the last admin")]
    LastAdmin,
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

pub async fn create(
    pool: &PgPool,
    org_id: Uuid,
    api_key_id: Uuid,
    role: Role,
) -> Result<OrgMember, sqlx::Error> {
    sqlx::query_as(
        "INSERT INTO org_members (org_id, api_key_id, role) \
         VALUES ($1, $2, $3) \
         RETURNING id, org_id, api_key_id, role, created_at",
    )
    .bind(org_id)
    .bind(api_key_id)
    .bind(role)
    .fetch_one(pool)
    .await
}

pub async fn get_role(
    pool: &PgPool,
    org_id: Uuid,
    api_key_id: Uuid,
) -> Result<Option<Role>, sqlx::Error> {
    let row: Option<(Role,)> =
        sqlx::query_as("SELECT role FROM org_members WHERE org_id = $1 AND api_key_id = $2")
            .bind(org_id)
            .bind(api_key_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(role,)| role))
}

pub async fn list_by_org(
    pool: &PgPool,
    org_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<OrgMember>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, org_id, api_key_id, role, created_at \
         FROM org_members WHERE org_id = $1 ORDER BY created_at LIMIT $2 OFFSET $3",
    )
    .bind(org_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
}

pub async fn delete(pool: &PgPool, org_id: Uuid, api_key_id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM org_members WHERE org_id = $1 AND api_key_id = $2")
        .bind(org_id)
        .bind(api_key_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn delete_unless_last_admin(
    pool: &PgPool,
    org_id: Uuid,
    api_key_id: Uuid,
) -> Result<bool, MemberError> {
    let mut tx = pool.begin().await?;

    let row: Option<(Role,)> = sqlx::query_as(
        "SELECT role FROM org_members WHERE org_id = $1 AND api_key_id = $2 FOR UPDATE",
    )
    .bind(org_id)
    .bind(api_key_id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some((role,)) = row else {
        return Ok(false);
    };

    if role == Role::Admin {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM org_members WHERE org_id = $1 AND role = 'admin'")
                .bind(org_id)
                .fetch_one(&mut *tx)
                .await?;

        if count <= 1 {
            return Err(MemberError::LastAdmin);
        }
    }

    sqlx::query("DELETE FROM org_members WHERE org_id = $1 AND api_key_id = $2")
        .bind(org_id)
        .bind(api_key_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(true)
}

pub async fn ensure_admin_exists(
    pool: &PgPool,
    org_id: Uuid,
    api_key_id: Uuid,
) -> Result<OrgMember, sqlx::Error> {
    sqlx::query_as(
        "INSERT INTO org_members (org_id, api_key_id, role) \
         VALUES ($1, $2, 'admin') \
         ON CONFLICT (org_id, api_key_id) DO UPDATE SET role = org_members.role \
         RETURNING id, org_id, api_key_id, role, created_at",
    )
    .bind(org_id)
    .bind(api_key_id)
    .fetch_one(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_prefix() -> String {
        format!("pb_{}", &Uuid::new_v4().to_string()[..8])
    }

    async fn setup_org_and_key(pool: &PgPool) -> (Uuid, Uuid) {
        let org = crate::db::organizations::create(pool, "Member Test Org")
            .await
            .unwrap();
        let key = crate::db::api_keys::create(pool, org.id, "hash_test", &unique_prefix(), None)
            .await
            .unwrap();
        (org.id, key.id)
    }

    #[tokio::test]
    #[ignore]
    async fn test_create_member() {
        let pool = crate::db::test_pool().await;
        let (org_id, key_id) = setup_org_and_key(&pool).await;

        let member = create(&pool, org_id, key_id, Role::Admin).await.unwrap();
        assert_eq!(member.org_id, org_id);
        assert_eq!(member.api_key_id, key_id);
        assert_eq!(member.role, Role::Admin);
    }

    #[tokio::test]
    #[ignore]
    async fn test_create_member_duplicate_fails() {
        let pool = crate::db::test_pool().await;
        let (org_id, key_id) = setup_org_and_key(&pool).await;

        create(&pool, org_id, key_id, Role::Admin).await.unwrap();
        let err = create(&pool, org_id, key_id, Role::Member).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    #[ignore]
    async fn test_get_role_exists() {
        let pool = crate::db::test_pool().await;
        let (org_id, key_id) = setup_org_and_key(&pool).await;

        create(&pool, org_id, key_id, Role::Member).await.unwrap();
        let role = get_role(&pool, org_id, key_id).await.unwrap();
        assert_eq!(role, Some(Role::Member));
    }

    #[tokio::test]
    #[ignore]
    async fn test_get_role_not_found() {
        let pool = crate::db::test_pool().await;
        let (org_id, _) = setup_org_and_key(&pool).await;
        let role = get_role(&pool, org_id, Uuid::new_v4()).await.unwrap();
        assert!(role.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_list_by_org() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "List Members Org")
            .await
            .unwrap();
        let k1 = crate::db::api_keys::create(&pool, org.id, "h1", &unique_prefix(), None)
            .await
            .unwrap();
        let k2 = crate::db::api_keys::create(&pool, org.id, "h2", &unique_prefix(), None)
            .await
            .unwrap();

        create(&pool, org.id, k1.id, Role::Admin).await.unwrap();
        create(&pool, org.id, k2.id, Role::Member).await.unwrap();

        let members = list_by_org(&pool, org.id, 100, 0).await.unwrap();
        assert_eq!(members.len(), 2);
    }

    #[tokio::test]
    #[ignore]
    async fn test_list_by_org_empty() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Empty Members Org")
            .await
            .unwrap();
        let members = list_by_org(&pool, org.id, 100, 0).await.unwrap();
        assert!(members.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_delete_member() {
        let pool = crate::db::test_pool().await;
        let (org_id, key_id) = setup_org_and_key(&pool).await;

        create(&pool, org_id, key_id, Role::Member).await.unwrap();
        assert!(delete(&pool, org_id, key_id).await.unwrap());
        assert!(get_role(&pool, org_id, key_id).await.unwrap().is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_delete_nonexistent_returns_false() {
        let pool = crate::db::test_pool().await;
        let (org_id, _) = setup_org_and_key(&pool).await;
        assert!(!delete(&pool, org_id, Uuid::new_v4()).await.unwrap());
    }

    #[tokio::test]
    #[ignore]
    async fn test_ensure_admin_exists_creates_new() {
        let pool = crate::db::test_pool().await;
        let (org_id, key_id) = setup_org_and_key(&pool).await;

        let member = ensure_admin_exists(&pool, org_id, key_id).await.unwrap();
        assert_eq!(member.role, Role::Admin);
    }

    #[tokio::test]
    #[ignore]
    async fn test_ensure_admin_exists_preserves_existing_role() {
        let pool = crate::db::test_pool().await;
        let (org_id, key_id) = setup_org_and_key(&pool).await;

        create(&pool, org_id, key_id, Role::Member).await.unwrap();
        let member = ensure_admin_exists(&pool, org_id, key_id).await.unwrap();
        assert_eq!(member.role, Role::Member);
    }
}
