use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{Permission, SendMode};

pub async fn upsert(
    pool: &PgPool,
    inbox_id: Uuid,
    send_mode: SendMode,
    rules: &serde_json::Value,
) -> Result<Permission, sqlx::Error> {
    sqlx::query_as(
        "INSERT INTO permissions (inbox_id, send_mode, rules) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (inbox_id) DO UPDATE SET send_mode = $2, rules = $3, updated_at = now() \
         RETURNING id, inbox_id, send_mode, rules, created_at, updated_at",
    )
    .bind(inbox_id)
    .bind(send_mode.to_string())
    .bind(rules)
    .fetch_one(pool)
    .await
}

pub async fn get_by_inbox(
    pool: &PgPool,
    inbox_id: Uuid,
) -> Result<Option<Permission>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, inbox_id, send_mode, rules, created_at, updated_at \
         FROM permissions WHERE inbox_id = $1",
    )
    .bind(inbox_id)
    .fetch_optional(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn test_permission_upsert_creates_new() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Perm Org")
            .await
            .unwrap();
        let email = format!("perm-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(&pool, org.id, &email, None, "native")
            .await
            .unwrap();

        let perm = upsert(&pool, inbox.id, SendMode::Approval, &serde_json::json!([]))
            .await
            .unwrap();
        assert_eq!(perm.inbox_id, inbox.id);
        assert_eq!(perm.send_mode, "approval");
        assert_eq!(perm.rules, serde_json::json!([]));
    }

    #[tokio::test]
    #[ignore]
    async fn test_permission_upsert_updates_existing() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Perm Upsert Org")
            .await
            .unwrap();
        let email = format!("perm-up-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(&pool, org.id, &email, None, "native")
            .await
            .unwrap();

        let p1 = upsert(&pool, inbox.id, SendMode::Approval, &serde_json::json!([]))
            .await
            .unwrap();
        let p2 = upsert(
            &pool,
            inbox.id,
            SendMode::Autonomous,
            &serde_json::json!([]),
        )
        .await
        .unwrap();
        assert_eq!(p1.id, p2.id);
        assert_eq!(p2.send_mode, "autonomous");
        assert!(p2.updated_at >= p1.updated_at);
    }

    #[tokio::test]
    #[ignore]
    async fn test_permission_get_by_inbox_exists() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Perm Get Org")
            .await
            .unwrap();
        let email = format!("perm-get-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(&pool, org.id, &email, None, "native")
            .await
            .unwrap();

        upsert(&pool, inbox.id, SendMode::Shadow, &serde_json::json!([]))
            .await
            .unwrap();

        let found = get_by_inbox(&pool, inbox.id).await.unwrap().unwrap();
        assert_eq!(found.send_mode, "shadow");
    }

    #[tokio::test]
    #[ignore]
    async fn test_permission_get_by_inbox_not_found() {
        let pool = crate::db::test_pool().await;
        let result = get_by_inbox(&pool, Uuid::new_v4()).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_permission_cascade_delete_with_inbox() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Perm Cascade Org")
            .await
            .unwrap();
        let email = format!("perm-cas-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(&pool, org.id, &email, None, "native")
            .await
            .unwrap();

        upsert(
            &pool,
            inbox.id,
            SendMode::Autonomous,
            &serde_json::json!([]),
        )
        .await
        .unwrap();
        crate::db::inboxes::delete(&pool, inbox.id).await.unwrap();

        let result = get_by_inbox(&pool, inbox.id).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_permission_unique_per_inbox() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Perm Unique Org")
            .await
            .unwrap();
        let email = format!("perm-uniq-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(&pool, org.id, &email, None, "native")
            .await
            .unwrap();

        // Two upserts produce the same row, not two rows
        upsert(&pool, inbox.id, SendMode::Approval, &serde_json::json!([]))
            .await
            .unwrap();
        upsert(&pool, inbox.id, SendMode::Shadow, &serde_json::json!([]))
            .await
            .unwrap();

        // Only one permission row for this inbox
        let rows: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM permissions WHERE inbox_id = $1")
            .bind(inbox.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows.0, 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_permission_all_send_modes() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Perm Modes Org")
            .await
            .unwrap();

        for mode in [
            SendMode::Shadow,
            SendMode::Approval,
            SendMode::AutoApprove,
            SendMode::Autonomous,
        ] {
            let email = format!("mode-{}-{}@example.com", mode, Uuid::new_v4());
            let inbox = crate::db::inboxes::create(&pool, org.id, &email, None, "native")
                .await
                .unwrap();

            let perm = upsert(&pool, inbox.id, mode, &serde_json::json!([]))
                .await
                .unwrap();
            assert_eq!(perm.send_mode, mode.to_string());
            assert_eq!(perm.mode(), mode);
        }
    }
}
