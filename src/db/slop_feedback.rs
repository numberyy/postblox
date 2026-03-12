use sqlx::PgPool;
use uuid::Uuid;

use crate::models::SlopFeedback;

pub async fn create(
    pool: &PgPool,
    org_id: Uuid,
    message_id: Uuid,
    is_slop: bool,
) -> Result<SlopFeedback, sqlx::Error> {
    sqlx::query_as(
        "INSERT INTO slop_feedback (org_id, message_id, is_slop) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (org_id, message_id) DO UPDATE SET is_slop = $3 \
         RETURNING id, org_id, message_id, is_slop, created_at",
    )
    .bind(org_id)
    .bind(message_id)
    .bind(is_slop)
    .fetch_one(pool)
    .await
}

pub async fn get_by_message(
    pool: &PgPool,
    org_id: Uuid,
    message_id: Uuid,
) -> Result<Option<SlopFeedback>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, org_id, message_id, is_slop, created_at \
         FROM slop_feedback WHERE org_id = $1 AND message_id = $2",
    )
    .bind(org_id)
    .bind(message_id)
    .fetch_optional(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_message(pool: &PgPool) -> (Uuid, Uuid) {
        let org = crate::db::organizations::create(pool, "Feedback Test Org")
            .await
            .unwrap();
        let email = format!("fb-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(pool, org.id, &email, None, "native")
            .await
            .unwrap();
        let cm = crate::models::CreateMessage {
            inbox_id: inbox.id,
            thread_id: None,
            message_id_header: Some(format!("<{}>", Uuid::new_v4())),
            in_reply_to: None,
            references_header: None,
            from_addr: "sender@example.com".into(),
            to_addrs: serde_json::json!(["rcpt@example.com"]),
            cc_addrs: None,
            subject: Some("Test".into()),
            text_body: Some("Hello".into()),
            html_body: None,
            extracted_text: None,
            direction: "inbound".into(),
            raw_headers: None,
        };
        let msg = crate::db::messages::create(pool, &cm).await.unwrap();
        (org.id, msg.id)
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_create_feedback() {
        let pool = crate::db::test_pool().await;
        let (org_id, msg_id) = setup_message(&pool).await;

        let fb = create(&pool, org_id, msg_id, true).await.unwrap();
        assert_eq!(fb.org_id, org_id);
        assert_eq!(fb.message_id, msg_id);
        assert!(fb.is_slop);
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_create_feedback_upsert_flips_is_slop() {
        let pool = crate::db::test_pool().await;
        let (org_id, msg_id) = setup_message(&pool).await;

        let fb1 = create(&pool, org_id, msg_id, true).await.unwrap();
        assert!(fb1.is_slop);

        let fb2 = create(&pool, org_id, msg_id, false).await.unwrap();
        assert!(!fb2.is_slop);
        assert_eq!(fb1.id, fb2.id);
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_get_by_message_exists() {
        let pool = crate::db::test_pool().await;
        let (org_id, msg_id) = setup_message(&pool).await;

        create(&pool, org_id, msg_id, false).await.unwrap();
        let fb = get_by_message(&pool, org_id, msg_id)
            .await
            .unwrap()
            .unwrap();
        assert!(!fb.is_slop);
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_get_by_message_not_found() {
        let pool = crate::db::test_pool().await;
        let (org_id, _msg_id) = setup_message(&pool).await;
        let result = get_by_message(&pool, org_id, Uuid::new_v4()).await.unwrap();
        assert!(result.is_none());
    }
}
