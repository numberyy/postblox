use pgvector::Vector;
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::Message;

pub async fn store_embedding(
    pool: &PgPool,
    message_id: Uuid,
    embedding: &[f32],
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE messages SET embedding = $2 WHERE id = $1")
        .bind(message_id)
        .bind(Vector::from(embedding.to_vec()))
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn search_similar(
    pool: &PgPool,
    org_id: Uuid,
    embedding: &[f32],
    limit: i64,
    offset: i64,
    threshold: f64,
    inbox_id: Option<Uuid>,
) -> Result<Vec<Message>, sqlx::Error> {
    let query = format!(
        "SELECT {} \
         FROM ( \
             SELECT m.*, m.embedding <=> $1::vector AS distance \
             FROM messages m \
             JOIN inboxes i ON m.inbox_id = i.id \
             WHERE i.org_id = $2 AND m.embedding IS NOT NULL \
             AND ($5::uuid IS NULL OR m.inbox_id = $5) \
         ) sub \
         WHERE 1 - distance >= $3 \
         ORDER BY distance \
         LIMIT $4 OFFSET $6",
        crate::db::messages::SELECT_COLS
    );
    sqlx::query_as(&query)
        .bind(Vector::from(embedding.to_vec()))
        .bind(org_id)
        .bind(threshold)
        .bind(limit)
        .bind(inbox_id)
        .bind(offset)
        .fetch_all(pool)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn setup_inbox(pool: &PgPool) -> crate::models::Inbox {
        let org = crate::db::organizations::create(pool, "Embed Test Org")
            .await
            .unwrap();
        let email = format!("embed-{}@example.com", Uuid::new_v4());
        crate::db::inboxes::create(pool, org.id, &email, None, crate::models::InboxType::Native)
            .await
            .unwrap()
    }

    async fn create_message(pool: &PgPool, inbox_id: Uuid, subject: &str) -> Message {
        let cm = crate::models::CreateMessage {
            inbox_id,
            thread_id: None,
            message_id_header: Some(format!("<embed-{}>", Uuid::new_v4())),
            in_reply_to: None,
            references_header: None,
            from_addr: "sender@example.com".into(),
            to_addrs: json!(["rcpt@example.com"]),
            cc_addrs: None,
            subject: Some(subject.into()),
            text_body: Some("test body".into()),
            html_body: None,
            extracted_text: None,
            direction: crate::models::Direction::Inbound,
            raw_headers: None,
        };
        crate::db::messages::create(pool, &cm).await.unwrap()
    }

    #[tokio::test]
    #[ignore]
    async fn test_store_embedding_and_search_similar() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let msg = create_message(&pool, inbox.id, "quarterly report").await;

        let embedding: Vec<f32> = (0..768).map(|i| (i as f32) / 768.0).collect();
        store_embedding(&pool, msg.id, &embedding).await.unwrap();

        let results = search_similar(&pool, inbox.org_id, &embedding, 10, 0, 0.5, None)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, msg.id);
    }

    #[tokio::test]
    #[ignore]
    async fn test_search_similar_threshold_filters_dissimilar() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let msg = create_message(&pool, inbox.id, "threshold test").await;

        let embedding: Vec<f32> = (0..768).map(|i| (i as f32) / 768.0).collect();
        store_embedding(&pool, msg.id, &embedding).await.unwrap();

        let orthogonal: Vec<f32> = (0..768)
            .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let results = search_similar(&pool, inbox.org_id, &orthogonal, 10, 0, 0.99, None)
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_search_similar_respects_org_boundary() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let msg = create_message(&pool, inbox.id, "org boundary test").await;

        let embedding: Vec<f32> = vec![1.0; 768];
        store_embedding(&pool, msg.id, &embedding).await.unwrap();

        let other_org = crate::db::organizations::create(&pool, "Other Embed Org")
            .await
            .unwrap();
        let results = search_similar(&pool, other_org.id, &embedding, 10, 0, 0.0, None)
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_store_embedding_overwrites_previous() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let msg = create_message(&pool, inbox.id, "overwrite test").await;

        let emb1: Vec<f32> = vec![1.0; 768];
        store_embedding(&pool, msg.id, &emb1).await.unwrap();

        let emb2: Vec<f32> = vec![0.5; 768];
        store_embedding(&pool, msg.id, &emb2).await.unwrap();

        let results = search_similar(&pool, inbox.org_id, &emb2, 10, 0, 0.99, None)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, msg.id);
    }

    #[tokio::test]
    #[ignore]
    async fn test_search_similar_orders_by_similarity() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let msg_close = create_message(&pool, inbox.id, "close match").await;
        let msg_far = create_message(&pool, inbox.id, "far match").await;

        let query: Vec<f32> = vec![1.0; 768];
        let close_emb: Vec<f32> = vec![0.99; 768];
        let far_emb: Vec<f32> = (0..768).map(|i| if i < 10 { 1.0 } else { 0.01 }).collect();

        store_embedding(&pool, msg_close.id, &close_emb)
            .await
            .unwrap();
        store_embedding(&pool, msg_far.id, &far_emb).await.unwrap();

        let results = search_similar(&pool, inbox.org_id, &query, 10, 0, 0.0, None)
            .await
            .unwrap();
        assert!(results.len() >= 2);
        assert_eq!(results[0].id, msg_close.id);
        assert_eq!(results[1].id, msg_far.id);
    }
}
