use sqlx::PgPool;
use uuid::Uuid;

use crate::models::Webhook;

pub async fn create(
    pool: &PgPool,
    org_id: Uuid,
    url: &str,
    events: &serde_json::Value,
    secret: &str,
) -> Result<Webhook, sqlx::Error> {
    sqlx::query_as(
        "INSERT INTO webhooks (org_id, url, events, secret) \
         VALUES ($1, $2, $3, $4) \
         RETURNING id, org_id, url, events, secret, active, created_at",
    )
    .bind(org_id)
    .bind(url)
    .bind(events)
    .bind(secret)
    .fetch_one(pool)
    .await
}

pub async fn get_by_id(pool: &PgPool, id: Uuid) -> Result<Option<Webhook>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, org_id, url, events, secret, active, created_at \
         FROM webhooks WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn list_by_org(pool: &PgPool, org_id: Uuid) -> Result<Vec<Webhook>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, org_id, url, events, secret, active, created_at \
         FROM webhooks WHERE org_id = $1 ORDER BY created_at",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await
}

pub async fn list_active_for_event(
    pool: &PgPool,
    org_id: Uuid,
    event_name: &str,
) -> Result<Vec<Webhook>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, org_id, url, events, secret, active, created_at \
         FROM webhooks WHERE org_id = $1 AND active = true AND events @> $2::jsonb",
    )
    .bind(org_id)
    .bind(serde_json::json!([event_name]))
    .fetch_all(pool)
    .await
}

pub async fn delete(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM webhooks WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Integration tests — require DATABASE_URL with migrations applied

    #[tokio::test]
    #[ignore]
    async fn test_webhook_create_and_get() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Webhook Org")
            .await
            .unwrap();

        let events = json!(["message.inbound"]);
        let wh = create(
            &pool,
            org.id,
            "https://example.com/hook",
            &events,
            "secret123",
        )
        .await
        .unwrap();
        assert_eq!(wh.url, "https://example.com/hook");
        assert!(wh.active);
        assert_eq!(wh.events, events);

        let fetched = get_by_id(&pool, wh.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, wh.id);
    }

    #[tokio::test]
    #[ignore]
    async fn test_webhook_list_by_org() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Webhook List Org")
            .await
            .unwrap();

        create(&pool, org.id, "https://a.com", &json!([]), "s1")
            .await
            .unwrap();
        create(&pool, org.id, "https://b.com", &json!([]), "s2")
            .await
            .unwrap();

        let hooks = list_by_org(&pool, org.id).await.unwrap();
        assert_eq!(hooks.len(), 2);
    }

    #[tokio::test]
    #[ignore]
    async fn test_webhook_list_by_org_empty() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Empty Webhook Org")
            .await
            .unwrap();
        let hooks = list_by_org(&pool, org.id).await.unwrap();
        assert!(hooks.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_webhook_list_active_for_event() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Event Filter Org")
            .await
            .unwrap();

        create(
            &pool,
            org.id,
            "https://a.com",
            &json!(["message.inbound", "message.outbound"]),
            "s1",
        )
        .await
        .unwrap();
        create(
            &pool,
            org.id,
            "https://b.com",
            &json!(["message.outbound"]),
            "s2",
        )
        .await
        .unwrap();

        let inbound = list_active_for_event(&pool, org.id, "message.inbound")
            .await
            .unwrap();
        assert_eq!(inbound.len(), 1);
        assert_eq!(inbound[0].url, "https://a.com");

        let outbound = list_active_for_event(&pool, org.id, "message.outbound")
            .await
            .unwrap();
        assert_eq!(outbound.len(), 2);
    }

    #[tokio::test]
    #[ignore]
    async fn test_webhook_list_active_for_event_no_match() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "No Match Org")
            .await
            .unwrap();
        create(&pool, org.id, "https://a.com", &json!(["other.event"]), "s")
            .await
            .unwrap();

        let result = list_active_for_event(&pool, org.id, "message.inbound")
            .await
            .unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_webhook_delete() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Del Webhook Org")
            .await
            .unwrap();
        let wh = create(&pool, org.id, "https://x.com", &json!([]), "s")
            .await
            .unwrap();

        assert!(delete(&pool, wh.id).await.unwrap());
        assert!(get_by_id(&pool, wh.id).await.unwrap().is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_webhook_delete_nonexistent() {
        let pool = crate::db::test_pool().await;
        assert!(!delete(&pool, Uuid::new_v4()).await.unwrap());
    }

    #[tokio::test]
    #[ignore]
    async fn test_webhook_events_empty_array() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Empty Events Org")
            .await
            .unwrap();
        let wh = create(&pool, org.id, "https://x.com", &json!([]), "s")
            .await
            .unwrap();
        assert_eq!(wh.events, json!([]));
    }
}
