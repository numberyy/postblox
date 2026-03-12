use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::AuditEntry;

const SELECT_COLS: &str = "id, org_id, inbox_id, action, actor, details, created_at";

pub async fn create_entry(
    pool: &PgPool,
    org_id: Uuid,
    inbox_id: Option<Uuid>,
    action: &str,
    actor: &str,
    details: serde_json::Value,
) -> Result<AuditEntry, sqlx::Error> {
    let query = format!(
        "INSERT INTO audit_log (org_id, inbox_id, action, actor, details) \
         VALUES ($1, $2, $3, $4, $5) \
         RETURNING {SELECT_COLS}"
    );
    sqlx::query_as(&query)
        .bind(org_id)
        .bind(inbox_id)
        .bind(action)
        .bind(actor)
        .bind(details)
        .fetch_one(pool)
        .await
}

#[allow(clippy::too_many_arguments)]
pub async fn list_entries(
    pool: &PgPool,
    org_id: Uuid,
    offset: i64,
    limit: i64,
    inbox_id_filter: Option<Uuid>,
    action_filter: Option<&str>,
    after: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
) -> Result<Vec<AuditEntry>, sqlx::Error> {
    let mut sql = format!("SELECT {SELECT_COLS} FROM audit_log WHERE org_id = $1");
    let mut param_idx = 2u32;

    if inbox_id_filter.is_some() {
        sql.push_str(&format!(" AND inbox_id = ${param_idx}"));
        param_idx += 1;
    }
    if action_filter.is_some() {
        sql.push_str(&format!(" AND action = ${param_idx}"));
        param_idx += 1;
    }
    if after.is_some() {
        sql.push_str(&format!(" AND created_at > ${param_idx}"));
        param_idx += 1;
    }
    if before.is_some() {
        sql.push_str(&format!(" AND created_at < ${param_idx}"));
        param_idx += 1;
    }

    sql.push_str(&format!(
        " ORDER BY created_at DESC LIMIT ${param_idx} OFFSET ${}",
        param_idx + 1
    ));

    let mut query = sqlx::query_as::<_, AuditEntry>(&sql).bind(org_id);

    if let Some(iid) = inbox_id_filter {
        query = query.bind(iid);
    }
    if let Some(act) = action_filter {
        query = query.bind(act);
    }
    if let Some(a) = after {
        query = query.bind(a);
    }
    if let Some(b) = before {
        query = query.bind(b);
    }

    query = query.bind(limit).bind(offset);

    query.fetch_all(pool).await
}

pub async fn count_entries(
    pool: &PgPool,
    org_id: Uuid,
    inbox_id_filter: Option<Uuid>,
    action_filter: Option<&str>,
    after: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
) -> Result<i64, sqlx::Error> {
    let mut sql = "SELECT COUNT(*) as count FROM audit_log WHERE org_id = $1".to_string();
    let mut param_idx = 2u32;

    if inbox_id_filter.is_some() {
        sql.push_str(&format!(" AND inbox_id = ${param_idx}"));
        param_idx += 1;
    }
    if action_filter.is_some() {
        sql.push_str(&format!(" AND action = ${param_idx}"));
        param_idx += 1;
    }
    if after.is_some() {
        sql.push_str(&format!(" AND created_at > ${param_idx}"));
        param_idx += 1;
    }
    if before.is_some() {
        sql.push_str(&format!(" AND created_at < ${param_idx}"));
        param_idx += 1;
    }
    let _ = param_idx;

    let mut query = sqlx::query_scalar::<_, i64>(&sql).bind(org_id);

    if let Some(iid) = inbox_id_filter {
        query = query.bind(iid);
    }
    if let Some(act) = action_filter {
        query = query.bind(act);
    }
    if let Some(a) = after {
        query = query.bind(a);
    }
    if let Some(b) = before {
        query = query.bind(b);
    }

    query.fetch_one(pool).await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_org(pool: &PgPool) -> (Uuid, Uuid) {
        let org = crate::db::organizations::create(pool, "Audit Test Org")
            .await
            .unwrap();
        let email = format!("audit-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(pool, org.id, &email, None, "native")
            .await
            .unwrap();
        (org.id, inbox.id)
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_create_entry_and_retrieve() {
        let pool = crate::db::test_pool().await;
        let (org_id, inbox_id) = setup_org(&pool).await;

        let entry = create_entry(
            &pool,
            org_id,
            Some(inbox_id),
            "message_sent",
            "api_key:pb_1234",
            serde_json::json!({"to": "user@example.com"}),
        )
        .await
        .unwrap();

        assert_eq!(entry.org_id, org_id);
        assert_eq!(entry.inbox_id, Some(inbox_id));
        assert_eq!(entry.action, "message_sent");
        assert_eq!(entry.actor, "api_key:pb_1234");
        assert_eq!(entry.details["to"], "user@example.com");
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_create_entry_without_inbox() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Audit No Inbox Org")
            .await
            .unwrap();

        let entry = create_entry(
            &pool,
            org.id,
            None,
            "domain_created",
            "system",
            serde_json::json!({"domain": "example.com"}),
        )
        .await
        .unwrap();

        assert!(entry.inbox_id.is_none());
        assert_eq!(entry.action, "domain_created");
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_list_entries_paginated() {
        let pool = crate::db::test_pool().await;
        let (org_id, inbox_id) = setup_org(&pool).await;

        for i in 0..5 {
            create_entry(
                &pool,
                org_id,
                Some(inbox_id),
                "message_sent",
                &format!("actor_{i}"),
                serde_json::json!({}),
            )
            .await
            .unwrap();
        }

        let page1 = list_entries(&pool, org_id, 0, 3, None, None, None, None)
            .await
            .unwrap();
        assert_eq!(page1.len(), 3);

        let page2 = list_entries(&pool, org_id, 3, 3, None, None, None, None)
            .await
            .unwrap();
        assert_eq!(page2.len(), 2);
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_list_entries_filter_by_action() {
        let pool = crate::db::test_pool().await;
        let (org_id, inbox_id) = setup_org(&pool).await;

        create_entry(
            &pool,
            org_id,
            Some(inbox_id),
            "message_sent",
            "actor",
            serde_json::json!({}),
        )
        .await
        .unwrap();
        create_entry(
            &pool,
            org_id,
            Some(inbox_id),
            "inbox_created",
            "actor",
            serde_json::json!({}),
        )
        .await
        .unwrap();

        let filtered = list_entries(
            &pool,
            org_id,
            0,
            100,
            None,
            Some("inbox_created"),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].action, "inbox_created");
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_list_entries_filter_by_inbox() {
        let pool = crate::db::test_pool().await;
        let (org_id, inbox_id) = setup_org(&pool).await;
        let email2 = format!("audit2-{}@example.com", Uuid::new_v4());
        let inbox2 = crate::db::inboxes::create(&pool, org_id, &email2, None, "native")
            .await
            .unwrap();

        create_entry(
            &pool,
            org_id,
            Some(inbox_id),
            "message_sent",
            "a",
            serde_json::json!({}),
        )
        .await
        .unwrap();
        create_entry(
            &pool,
            org_id,
            Some(inbox2.id),
            "message_sent",
            "a",
            serde_json::json!({}),
        )
        .await
        .unwrap();

        let filtered = list_entries(&pool, org_id, 0, 100, Some(inbox_id), None, None, None)
            .await
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].inbox_id, Some(inbox_id));
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_count_entries_basic() {
        let pool = crate::db::test_pool().await;
        let (org_id, inbox_id) = setup_org(&pool).await;

        for _ in 0..3 {
            create_entry(
                &pool,
                org_id,
                Some(inbox_id),
                "message_sent",
                "a",
                serde_json::json!({}),
            )
            .await
            .unwrap();
        }

        let count = count_entries(&pool, org_id, None, None, None, None)
            .await
            .unwrap();
        assert!(count >= 3);
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_count_entries_with_action_filter() {
        let pool = crate::db::test_pool().await;
        let (org_id, inbox_id) = setup_org(&pool).await;

        create_entry(
            &pool,
            org_id,
            Some(inbox_id),
            "message_sent",
            "a",
            serde_json::json!({}),
        )
        .await
        .unwrap();
        create_entry(
            &pool,
            org_id,
            Some(inbox_id),
            "inbox_created",
            "a",
            serde_json::json!({}),
        )
        .await
        .unwrap();

        let count = count_entries(&pool, org_id, None, Some("inbox_created"), None, None)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_list_entries_respects_org_boundary() {
        let pool = crate::db::test_pool().await;
        let (org_id, inbox_id) = setup_org(&pool).await;
        let other_org = crate::db::organizations::create(&pool, "Other Audit Org")
            .await
            .unwrap();

        create_entry(
            &pool,
            org_id,
            Some(inbox_id),
            "message_sent",
            "a",
            serde_json::json!({}),
        )
        .await
        .unwrap();

        let entries = list_entries(&pool, other_org.id, 0, 100, None, None, None, None)
            .await
            .unwrap();
        assert!(entries.is_empty());
    }
}
