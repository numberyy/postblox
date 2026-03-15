use sqlx::PgPool;
use uuid::Uuid;

use crate::models::DeliveryStatus;

pub async fn create_status(
    pool: &PgPool,
    message_id: Uuid,
    status: crate::models::DeliveryStatusType,
    bounce_type: Option<crate::models::BounceType>,
    details: Option<serde_json::Value>,
) -> Result<DeliveryStatus, sqlx::Error> {
    sqlx::query_as(
        "INSERT INTO delivery_status (message_id, status, bounce_type, details) \
         VALUES ($1, $2, $3, $4) \
         RETURNING id, message_id, status, bounce_type, details, created_at",
    )
    .bind(message_id)
    .bind(status)
    .bind(bounce_type)
    .bind(details)
    .fetch_one(pool)
    .await
}

pub async fn get_by_message(
    pool: &PgPool,
    message_id: Uuid,
) -> Result<Vec<DeliveryStatus>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, message_id, status, bounce_type, details, created_at \
         FROM delivery_status WHERE message_id = $1 ORDER BY created_at",
    )
    .bind(message_id)
    .fetch_all(pool)
    .await
}

pub async fn count_hard_bounces_for_inbox(
    pool: &PgPool,
    inbox_id: Uuid,
) -> Result<i64, sqlx::Error> {
    let row: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM delivery_status ds \
         JOIN messages m ON m.id = ds.message_id \
         WHERE m.inbox_id = $1 \
         AND ds.status = 'bounced' \
         AND ds.bounce_type = 'hard' \
         AND ds.created_at > now() - interval '24 hours'",
    )
    .bind(inbox_id)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn test_create_status_delivered_roundtrip() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Bounce Org")
            .await
            .unwrap();
        let email = format!("bounce-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(
            &pool,
            org.id,
            &email,
            None,
            crate::models::InboxType::Native,
        )
        .await
        .unwrap();
        let msg = crate::db::messages::create(
            &pool,
            &crate::models::CreateMessage {
                inbox_id: inbox.id,
                thread_id: None,
                message_id_header: None,
                in_reply_to: None,
                references_header: None,
                from_addr: email.clone(),
                to_addrs: serde_json::json!(["user@example.com"]),
                cc_addrs: None,
                subject: Some("Test".into()),
                text_body: Some("Body".into()),
                html_body: None,
                extracted_text: None,
                direction: crate::models::Direction::Outbound,
                raw_headers: None,
            },
        )
        .await
        .unwrap();

        let ds = create_status(
            &pool,
            msg.id,
            crate::models::DeliveryStatusType::Delivered,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(ds.message_id, msg.id);
        assert_eq!(ds.status, crate::models::DeliveryStatusType::Delivered);
        assert!(ds.bounce_type.is_none());

        let list = get_by_message(&pool, msg.id).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, ds.id);
    }

    #[tokio::test]
    #[ignore]
    async fn test_create_status_bounced_with_details() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Bounce Details Org")
            .await
            .unwrap();
        let email = format!("bd-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(
            &pool,
            org.id,
            &email,
            None,
            crate::models::InboxType::Native,
        )
        .await
        .unwrap();
        let msg = crate::db::messages::create(
            &pool,
            &crate::models::CreateMessage {
                inbox_id: inbox.id,
                thread_id: None,
                message_id_header: None,
                in_reply_to: None,
                references_header: None,
                from_addr: email.clone(),
                to_addrs: serde_json::json!(["user@example.com"]),
                cc_addrs: None,
                subject: Some("Test".into()),
                text_body: Some("Body".into()),
                html_body: None,
                extracted_text: None,
                direction: crate::models::Direction::Outbound,
                raw_headers: None,
            },
        )
        .await
        .unwrap();

        let details = serde_json::json!({"smtp_code": 550, "reason": "mailbox not found"});
        let ds = create_status(
            &pool,
            msg.id,
            crate::models::DeliveryStatusType::Bounced,
            Some(crate::models::BounceType::Hard),
            Some(details.clone()),
        )
        .await
        .unwrap();
        assert_eq!(ds.status, crate::models::DeliveryStatusType::Bounced);
        assert_eq!(ds.bounce_type, Some(crate::models::BounceType::Hard));
        assert_eq!(ds.details, Some(details));
    }

    #[tokio::test]
    #[ignore]
    async fn test_count_hard_bounces_for_inbox() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Count Bounces Org")
            .await
            .unwrap();
        let email = format!("cnt-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(
            &pool,
            org.id,
            &email,
            None,
            crate::models::InboxType::Native,
        )
        .await
        .unwrap();

        let mut msg_ids = Vec::new();
        for i in 0..6 {
            let msg = crate::db::messages::create(
                &pool,
                &crate::models::CreateMessage {
                    inbox_id: inbox.id,
                    thread_id: None,
                    message_id_header: Some(format!("bounce-{i}@test")),
                    in_reply_to: None,
                    references_header: None,
                    from_addr: email.clone(),
                    to_addrs: serde_json::json!(["user@example.com"]),
                    cc_addrs: None,
                    subject: Some(format!("Bounce {i}")),
                    text_body: Some("Body".into()),
                    html_body: None,
                    extracted_text: None,
                    direction: crate::models::Direction::Outbound,
                    raw_headers: None,
                },
            )
            .await
            .unwrap();
            msg_ids.push(msg.id);
        }

        // 4 hard bounces
        for &mid in &msg_ids[..4] {
            create_status(
                &pool,
                mid,
                crate::models::DeliveryStatusType::Bounced,
                Some(crate::models::BounceType::Hard),
                None,
            )
            .await
            .unwrap();
        }
        // 1 soft bounce (should not count)
        create_status(
            &pool,
            msg_ids[4],
            crate::models::DeliveryStatusType::Bounced,
            Some(crate::models::BounceType::Soft),
            None,
        )
        .await
        .unwrap();
        // 1 delivered (should not count)
        create_status(
            &pool,
            msg_ids[5],
            crate::models::DeliveryStatusType::Delivered,
            None,
            None,
        )
        .await
        .unwrap();

        let count = count_hard_bounces_for_inbox(&pool, inbox.id).await.unwrap();
        assert_eq!(count, 4);
    }

    #[tokio::test]
    #[ignore]
    async fn test_get_by_message_empty() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Empty DS Org")
            .await
            .unwrap();
        let email = format!("empty-ds-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(
            &pool,
            org.id,
            &email,
            None,
            crate::models::InboxType::Native,
        )
        .await
        .unwrap();
        let msg = crate::db::messages::create(
            &pool,
            &crate::models::CreateMessage {
                inbox_id: inbox.id,
                thread_id: None,
                message_id_header: None,
                in_reply_to: None,
                references_header: None,
                from_addr: email.clone(),
                to_addrs: serde_json::json!(["user@example.com"]),
                cc_addrs: None,
                subject: None,
                text_body: None,
                html_body: None,
                extracted_text: None,
                direction: crate::models::Direction::Outbound,
                raw_headers: None,
            },
        )
        .await
        .unwrap();

        let list = get_by_message(&pool, msg.id).await.unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_count_hard_bounces_excludes_other_inboxes() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Isolation Org")
            .await
            .unwrap();
        let e1 = format!("iso1-{}@example.com", Uuid::new_v4());
        let e2 = format!("iso2-{}@example.com", Uuid::new_v4());
        let inbox1 =
            crate::db::inboxes::create(&pool, org.id, &e1, None, crate::models::InboxType::Native)
                .await
                .unwrap();
        let inbox2 =
            crate::db::inboxes::create(&pool, org.id, &e2, None, crate::models::InboxType::Native)
                .await
                .unwrap();

        // Bounce on inbox2
        let msg = crate::db::messages::create(
            &pool,
            &crate::models::CreateMessage {
                inbox_id: inbox2.id,
                thread_id: None,
                message_id_header: None,
                in_reply_to: None,
                references_header: None,
                from_addr: e2.clone(),
                to_addrs: serde_json::json!(["user@example.com"]),
                cc_addrs: None,
                subject: None,
                text_body: None,
                html_body: None,
                extracted_text: None,
                direction: crate::models::Direction::Outbound,
                raw_headers: None,
            },
        )
        .await
        .unwrap();
        create_status(
            &pool,
            msg.id,
            crate::models::DeliveryStatusType::Bounced,
            Some(crate::models::BounceType::Hard),
            None,
        )
        .await
        .unwrap();

        // inbox1 should have 0
        let count = count_hard_bounces_for_inbox(&pool, inbox1.id)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}
