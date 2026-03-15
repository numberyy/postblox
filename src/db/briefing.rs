use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Serialize, sqlx::FromRow)]
pub struct InboxStats {
    pub inbox_id: Uuid,
    pub inbox_email: String,
    pub received: i64,
    pub sent: i64,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct SenderCount {
    pub address: String,
    pub count: i64,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct SubjectCount {
    pub subject: String,
    pub count: i64,
}

pub async fn stats_by_inbox(
    pool: &PgPool,
    org_id: Uuid,
    since: DateTime<Utc>,
) -> Result<Vec<InboxStats>, sqlx::Error> {
    sqlx::query_as(
        "SELECT i.id AS inbox_id, i.email AS inbox_email, \
         COUNT(*) FILTER (WHERE m.direction = 'inbound') AS received, \
         COUNT(*) FILTER (WHERE m.direction = 'outbound') AS sent \
         FROM inboxes i \
         LEFT JOIN messages m ON m.inbox_id = i.id AND m.created_at >= $2 \
         WHERE i.org_id = $1 \
         GROUP BY i.id, i.email \
         ORDER BY COUNT(m.id) DESC",
    )
    .bind(org_id)
    .bind(since)
    .fetch_all(pool)
    .await
}

pub async fn top_senders(
    pool: &PgPool,
    org_id: Uuid,
    since: DateTime<Utc>,
) -> Result<Vec<SenderCount>, sqlx::Error> {
    sqlx::query_as(
        "SELECT m.from_addr AS address, COUNT(*) AS count \
         FROM messages m \
         JOIN inboxes i ON m.inbox_id = i.id \
         WHERE i.org_id = $1 AND m.created_at >= $2 AND m.direction = 'inbound' \
         GROUP BY m.from_addr \
         ORDER BY count DESC \
         LIMIT 10",
    )
    .bind(org_id)
    .bind(since)
    .fetch_all(pool)
    .await
}

#[derive(Serialize, sqlx::FromRow)]
pub struct TriageCount {
    pub status: String,
    pub count: i64,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct SlopSender {
    pub sender_email: String,
    pub total_messages: i32,
    pub slop_count: i32,
}

pub async fn count_by_triage_status(
    pool: &PgPool,
    org_id: Uuid,
) -> Result<Vec<TriageCount>, sqlx::Error> {
    sqlx::query_as(
        "SELECT COALESCE(triage_status, 'unclassified') AS status, COUNT(*) AS count \
         FROM messages m JOIN inboxes i ON m.inbox_id = i.id \
         WHERE i.org_id = $1 \
         GROUP BY triage_status \
         ORDER BY count DESC",
    )
    .bind(org_id)
    .fetch_all(pool)
    .await
}

pub async fn top_slop_senders(
    pool: &PgPool,
    org_id: Uuid,
    limit: i64,
) -> Result<Vec<SlopSender>, sqlx::Error> {
    sqlx::query_as(
        "SELECT sender_email, total_messages, slop_count \
         FROM sender_reputation \
         WHERE org_id = $1 AND slop_count > 0 \
         ORDER BY slop_count DESC \
         LIMIT $2",
    )
    .bind(org_id)
    .bind(limit)
    .fetch_all(pool)
    .await
}

pub async fn top_subjects(
    pool: &PgPool,
    org_id: Uuid,
    since: DateTime<Utc>,
) -> Result<Vec<SubjectCount>, sqlx::Error> {
    sqlx::query_as(
        "SELECT m.subject, COUNT(*) AS count \
         FROM messages m \
         JOIN inboxes i ON m.inbox_id = i.id \
         WHERE i.org_id = $1 AND m.created_at >= $2 AND m.subject IS NOT NULL \
         GROUP BY m.subject \
         ORDER BY count DESC \
         LIMIT 10",
    )
    .bind(org_id)
    .bind(since)
    .fetch_all(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    #[ignore]
    async fn test_briefing_stats_by_inbox_empty_org() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Briefing Empty Org")
            .await
            .unwrap();
        let since = Utc::now() - chrono::Duration::hours(24);
        let stats = stats_by_inbox(&pool, org.id, since).await.unwrap();
        assert!(stats.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_briefing_top_senders_empty() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Briefing Senders Empty")
            .await
            .unwrap();
        let since = Utc::now() - chrono::Duration::hours(24);
        let senders = top_senders(&pool, org.id, since).await.unwrap();
        assert!(senders.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_briefing_top_subjects_empty() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Briefing Subjects Empty")
            .await
            .unwrap();
        let since = Utc::now() - chrono::Duration::hours(24);
        let subjects = top_subjects(&pool, org.id, since).await.unwrap();
        assert!(subjects.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_briefing_stats_by_inbox_counts_messages() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Briefing Count Org")
            .await
            .unwrap();
        let email = format!("brief-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(
            &pool,
            org.id,
            &email,
            None,
            crate::models::InboxType::Native,
        )
        .await
        .unwrap();

        let cm = crate::models::CreateMessage {
            inbox_id: inbox.id,
            thread_id: None,
            message_id_header: Some(format!("<brief-{}>", Uuid::new_v4())),
            in_reply_to: None,
            references_header: None,
            from_addr: "sender@example.com".into(),
            to_addrs: json!(["rcpt@example.com"]),
            cc_addrs: None,
            subject: Some("Test".into()),
            text_body: Some("Hello".into()),
            html_body: None,
            extracted_text: None,
            direction: crate::models::Direction::Inbound,
            raw_headers: None,
        };
        crate::db::messages::create(&pool, &cm).await.unwrap();

        let since = Utc::now() - chrono::Duration::hours(1);
        let stats = stats_by_inbox(&pool, org.id, since).await.unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].inbox_email, email);
        assert_eq!(stats[0].received, 1);
        assert_eq!(stats[0].sent, 0);
    }

    #[tokio::test]
    #[ignore]
    async fn test_briefing_stats_by_inbox_includes_inbox_with_no_messages() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Briefing NoMsg Org")
            .await
            .unwrap();
        let email = format!("brief-nomsg-{}@example.com", Uuid::new_v4());
        crate::db::inboxes::create(
            &pool,
            org.id,
            &email,
            None,
            crate::models::InboxType::Native,
        )
        .await
        .unwrap();

        let since = Utc::now() - chrono::Duration::hours(1);
        let stats = stats_by_inbox(&pool, org.id, since).await.unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].received, 0);
        assert_eq!(stats[0].sent, 0);
    }

    #[tokio::test]
    #[ignore]
    #[allow(clippy::too_many_lines)]
    async fn test_briefing_top_senders_returns_correct_order() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Briefing Senders Org")
            .await
            .unwrap();
        let email = format!("brief-s-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(
            &pool,
            org.id,
            &email,
            None,
            crate::models::InboxType::Native,
        )
        .await
        .unwrap();

        for (from, count) in [("alice@example.com", 2), ("bob@example.com", 1)] {
            for i in 0..count {
                let cm = crate::models::CreateMessage {
                    inbox_id: inbox.id,
                    thread_id: None,
                    message_id_header: Some(format!("<{from}-{i}-{}>", Uuid::new_v4())),
                    in_reply_to: None,
                    references_header: None,
                    from_addr: from.into(),
                    to_addrs: json!([email]),
                    cc_addrs: None,
                    subject: Some("Test".into()),
                    text_body: None,
                    html_body: None,
                    extracted_text: None,
                    direction: crate::models::Direction::Inbound,
                    raw_headers: None,
                };
                crate::db::messages::create(&pool, &cm).await.unwrap();
            }
        }

        let since = Utc::now() - chrono::Duration::hours(1);
        let senders = top_senders(&pool, org.id, since).await.unwrap();
        assert_eq!(senders[0].address, "alice@example.com");
        assert_eq!(senders[0].count, 2);
        assert_eq!(senders[1].address, "bob@example.com");
        assert_eq!(senders[1].count, 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_briefing_top_senders_excludes_outbound() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Briefing Outbound Org")
            .await
            .unwrap();
        let email = format!("brief-out-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(
            &pool,
            org.id,
            &email,
            None,
            crate::models::InboxType::Native,
        )
        .await
        .unwrap();

        let cm = crate::models::CreateMessage {
            inbox_id: inbox.id,
            thread_id: None,
            message_id_header: Some(format!("<out-{}>", Uuid::new_v4())),
            in_reply_to: None,
            references_header: None,
            from_addr: email.clone(),
            to_addrs: json!(["external@example.com"]),
            cc_addrs: None,
            subject: Some("Outbound".into()),
            text_body: None,
            html_body: None,
            extracted_text: None,
            direction: crate::models::Direction::Outbound,
            raw_headers: None,
        };
        crate::db::messages::create(&pool, &cm).await.unwrap();

        let since = Utc::now() - chrono::Duration::hours(1);
        let senders = top_senders(&pool, org.id, since).await.unwrap();
        assert!(senders.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_briefing_respects_time_boundary() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Briefing Time Org")
            .await
            .unwrap();
        let email = format!("brief-t-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(
            &pool,
            org.id,
            &email,
            None,
            crate::models::InboxType::Native,
        )
        .await
        .unwrap();

        let cm = crate::models::CreateMessage {
            inbox_id: inbox.id,
            thread_id: None,
            message_id_header: Some(format!("<brief-time-{}>", Uuid::new_v4())),
            in_reply_to: None,
            references_header: None,
            from_addr: "old@example.com".into(),
            to_addrs: json!([email]),
            cc_addrs: None,
            subject: Some("Old message".into()),
            text_body: None,
            html_body: None,
            extracted_text: None,
            direction: crate::models::Direction::Inbound,
            raw_headers: None,
        };
        crate::db::messages::create(&pool, &cm).await.unwrap();

        let since = Utc::now() + chrono::Duration::hours(1);
        let senders = top_senders(&pool, org.id, since).await.unwrap();
        assert!(senders.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_briefing_respects_org_boundary() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Briefing Org A")
            .await
            .unwrap();
        let email = format!("brief-org-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(
            &pool,
            org.id,
            &email,
            None,
            crate::models::InboxType::Native,
        )
        .await
        .unwrap();

        let cm = crate::models::CreateMessage {
            inbox_id: inbox.id,
            thread_id: None,
            message_id_header: Some(format!("<brief-orgb-{}>", Uuid::new_v4())),
            in_reply_to: None,
            references_header: None,
            from_addr: "someone@example.com".into(),
            to_addrs: json!([email]),
            cc_addrs: None,
            subject: Some("Scoped".into()),
            text_body: None,
            html_body: None,
            extracted_text: None,
            direction: crate::models::Direction::Inbound,
            raw_headers: None,
        };
        crate::db::messages::create(&pool, &cm).await.unwrap();

        let other_org = crate::db::organizations::create(&pool, "Briefing Org B")
            .await
            .unwrap();
        let since = Utc::now() - chrono::Duration::hours(1);
        let senders = top_senders(&pool, other_org.id, since).await.unwrap();
        assert!(senders.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_briefing_top_subjects_skips_null() {
        let pool = crate::db::test_pool().await;
        let org = crate::db::organizations::create(&pool, "Briefing NullSubj Org")
            .await
            .unwrap();
        let email = format!("brief-ns-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(
            &pool,
            org.id,
            &email,
            None,
            crate::models::InboxType::Native,
        )
        .await
        .unwrap();

        let cm = crate::models::CreateMessage {
            inbox_id: inbox.id,
            thread_id: None,
            message_id_header: Some(format!("<null-subj-{}>", Uuid::new_v4())),
            in_reply_to: None,
            references_header: None,
            from_addr: "nosubj@example.com".into(),
            to_addrs: json!([email]),
            cc_addrs: None,
            subject: None,
            text_body: Some("No subject".into()),
            html_body: None,
            extracted_text: None,
            direction: crate::models::Direction::Inbound,
            raw_headers: None,
        };
        crate::db::messages::create(&pool, &cm).await.unwrap();

        let since = Utc::now() - chrono::Duration::hours(1);
        let subjects = top_subjects(&pool, org.id, since).await.unwrap();
        assert!(subjects.is_empty());
    }
}
