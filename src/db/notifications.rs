use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{CreateNotificationConfig, NotificationConfig};

const SELECT_COLS: &str = "id, org_id, provider, config, active, created_at";

pub async fn list_active(
    pool: &PgPool,
    org_id: Uuid,
) -> Result<Vec<NotificationConfig>, sqlx::Error> {
    sqlx::query_as(&format!(
        "SELECT {SELECT_COLS} FROM notification_config \
         WHERE org_id = $1 AND active = true \
         ORDER BY created_at DESC"
    ))
    .bind(org_id)
    .fetch_all(pool)
    .await
}

pub async fn create(
    pool: &PgPool,
    input: &CreateNotificationConfig,
) -> Result<NotificationConfig, sqlx::Error> {
    sqlx::query_as(&format!(
        "INSERT INTO notification_config (org_id, provider, config) \
         VALUES ($1, $2, $3) \
         RETURNING {SELECT_COLS}"
    ))
    .bind(input.org_id)
    .bind(input.provider.to_string())
    .bind(&input.config)
    .fetch_one(pool)
    .await
}

pub async fn delete(pool: &PgPool, id: Uuid, org_id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM notification_config WHERE id = $1 AND org_id = $2")
        .bind(id)
        .bind(org_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::NotificationProvider;

    async fn setup_org(pool: &PgPool) -> Uuid {
        let org = crate::db::organizations::create(pool, "Notif Test Org")
            .await
            .unwrap();
        org.id
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_create_notification_config() {
        let pool = crate::db::test_pool().await;
        let org_id = setup_org(&pool).await;

        let input = CreateNotificationConfig {
            org_id,
            provider: NotificationProvider::Ntfy,
            config: serde_json::json!({"url": "https://ntfy.sh/postblox"}),
        };
        let nc = create(&pool, &input).await.unwrap();
        assert_eq!(nc.org_id, org_id);
        assert_eq!(nc.provider, "ntfy");
        assert!(nc.active);
        assert_eq!(nc.config["url"], "https://ntfy.sh/postblox");
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_list_active_returns_only_active() {
        let pool = crate::db::test_pool().await;
        let org_id = setup_org(&pool).await;

        let input = CreateNotificationConfig {
            org_id,
            provider: NotificationProvider::Webhook,
            config: serde_json::json!({"url": "https://example.com/hook"}),
        };
        let nc = create(&pool, &input).await.unwrap();

        let active = list_active(&pool, org_id).await.unwrap();
        assert!(active.iter().any(|c| c.id == nc.id));

        // Deactivate
        sqlx::query("UPDATE notification_config SET active = false WHERE id = $1")
            .bind(nc.id)
            .execute(&pool)
            .await
            .unwrap();

        let active = list_active(&pool, org_id).await.unwrap();
        assert!(!active.iter().any(|c| c.id == nc.id));
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_list_active_org_scoped() {
        let pool = crate::db::test_pool().await;
        let org_id = setup_org(&pool).await;
        let other_org_id = setup_org(&pool).await;

        let input = CreateNotificationConfig {
            org_id,
            provider: NotificationProvider::Email,
            config: serde_json::json!({}),
        };
        create(&pool, &input).await.unwrap();

        let other_active = list_active(&pool, other_org_id).await.unwrap();
        assert!(other_active.is_empty());
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_delete_notification_config() {
        let pool = crate::db::test_pool().await;
        let org_id = setup_org(&pool).await;

        let input = CreateNotificationConfig {
            org_id,
            provider: NotificationProvider::Ntfy,
            config: serde_json::json!({}),
        };
        let nc = create(&pool, &input).await.unwrap();

        let deleted = delete(&pool, nc.id, org_id).await.unwrap();
        assert!(deleted);

        let active = list_active(&pool, org_id).await.unwrap();
        assert!(!active.iter().any(|c| c.id == nc.id));
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_delete_wrong_org_returns_false() {
        let pool = crate::db::test_pool().await;
        let org_id = setup_org(&pool).await;
        let other_org_id = setup_org(&pool).await;

        let input = CreateNotificationConfig {
            org_id,
            provider: NotificationProvider::Ntfy,
            config: serde_json::json!({}),
        };
        let nc = create(&pool, &input).await.unwrap();

        let deleted = delete(&pool, nc.id, other_org_id).await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_delete_nonexistent_returns_false() {
        let pool = crate::db::test_pool().await;
        let org_id = setup_org(&pool).await;

        let deleted = delete(&pool, Uuid::new_v4(), org_id).await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_create_multiple_providers() {
        let pool = crate::db::test_pool().await;
        let org_id = setup_org(&pool).await;

        for provider in [
            NotificationProvider::Ntfy,
            NotificationProvider::Email,
            NotificationProvider::Webhook,
        ] {
            let input = CreateNotificationConfig {
                org_id,
                provider,
                config: serde_json::json!({}),
            };
            create(&pool, &input).await.unwrap();
        }

        let active = list_active(&pool, org_id).await.unwrap();
        assert!(active.len() >= 3);
    }
}
