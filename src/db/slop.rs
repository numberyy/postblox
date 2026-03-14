use sqlx::PgPool;
use uuid::Uuid;

use crate::models::SenderReputation;

pub struct SlopFields<'a> {
    pub score: f32,
    pub signals: &'a serde_json::Value,
    pub category: &'a str,
    pub priority: &'a str,
    pub triage_status: &'a str,
    pub requires_action: bool,
}

pub async fn update_slop_fields(
    pool: &PgPool,
    message_id: Uuid,
    fields: &SlopFields<'_>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE messages SET slop_score = $1, slop_signals = $2, category = $3, \
         priority = $4, triage_status = $5, requires_action = $6 WHERE id = $7",
    )
    .bind(fields.score)
    .bind(fields.signals)
    .bind(fields.category)
    .bind(fields.priority)
    .bind(fields.triage_status)
    .bind(fields.requires_action)
    .bind(message_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_sender_reputation(
    pool: &PgPool,
    org_id: Uuid,
    sender_email: &str,
) -> Result<Option<SenderReputation>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, org_id, sender_email, total_messages, slop_count, \
         last_seen_at, created_at \
         FROM sender_reputation WHERE org_id = $1 AND sender_email = $2",
    )
    .bind(org_id)
    .bind(sender_email)
    .fetch_optional(pool)
    .await
}

pub async fn upsert_sender_reputation(
    pool: &PgPool,
    org_id: Uuid,
    sender_email: &str,
    is_slop: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO sender_reputation (org_id, sender_email, total_messages, slop_count) \
         VALUES ($1, $2, 1, CASE WHEN $3 THEN 1 ELSE 0 END) \
         ON CONFLICT (org_id, sender_email) DO UPDATE SET \
         total_messages = sender_reputation.total_messages + 1, \
         slop_count = sender_reputation.slop_count + CASE WHEN $3 THEN 1 ELSE 0 END, \
         last_seen_at = now()",
    )
    .bind(org_id)
    .bind(sender_email)
    .bind(is_slop)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_org(pool: &PgPool) -> crate::models::Organization {
        crate::db::organizations::create(pool, "Slop Test Org")
            .await
            .unwrap()
    }

    async fn setup_message(pool: &PgPool) -> (crate::models::Organization, crate::models::Message) {
        let org = setup_org(pool).await;
        let email = format!("slop-{}@example.com", Uuid::new_v4());
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
            direction: crate::models::Direction::Inbound,
            raw_headers: None,
        };
        let msg = crate::db::messages::create(pool, &cm).await.unwrap();
        (org, msg)
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_update_slop_fields_and_read_back() {
        let pool = crate::db::test_pool().await;
        let (_org, msg) = setup_message(&pool).await;

        let signals = serde_json::json!(["list-unsubscribe", "noreply-sender"]);
        let fields = SlopFields {
            score: 0.45,
            signals: &signals,
            category: "commercial",
            priority: "normal",
            triage_status: "inbox",
            requires_action: false,
        };
        update_slop_fields(&pool, msg.id, &fields).await.unwrap();

        let updated = crate::db::messages::get_by_id(&pool, msg.id)
            .await
            .unwrap()
            .unwrap();
        assert!((updated.slop_score.unwrap() - 0.45).abs() < f32::EPSILON);
        assert_eq!(updated.category.as_deref(), Some("commercial"));
        assert_eq!(updated.priority.as_deref(), Some("normal"));
        assert_eq!(updated.triage_status.as_deref(), Some("inbox"));
        assert_eq!(updated.requires_action, Some(false));
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_upsert_sender_reputation_new() {
        let pool = crate::db::test_pool().await;
        let org = setup_org(&pool).await;
        let email = format!("new-sender-{}@example.com", Uuid::new_v4());

        upsert_sender_reputation(&pool, org.id, &email, false)
            .await
            .unwrap();

        let rep = get_sender_reputation(&pool, org.id, &email)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rep.total_messages, 1);
        assert_eq!(rep.slop_count, 0);
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_upsert_sender_reputation_increment() {
        let pool = crate::db::test_pool().await;
        let org = setup_org(&pool).await;
        let email = format!("repeat-{}@example.com", Uuid::new_v4());

        upsert_sender_reputation(&pool, org.id, &email, true)
            .await
            .unwrap();
        upsert_sender_reputation(&pool, org.id, &email, false)
            .await
            .unwrap();
        upsert_sender_reputation(&pool, org.id, &email, true)
            .await
            .unwrap();

        let rep = get_sender_reputation(&pool, org.id, &email)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rep.total_messages, 3);
        assert_eq!(rep.slop_count, 2);
    }

    #[tokio::test]
    #[ignore] // needs real DB
    async fn test_get_sender_reputation_nonexistent() {
        let pool = crate::db::test_pool().await;
        let org = setup_org(&pool).await;
        let rep = get_sender_reputation(&pool, org.id, "nobody@nowhere.com")
            .await
            .unwrap();
        assert!(rep.is_none());
    }
}
