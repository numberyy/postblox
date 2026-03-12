use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{CreateMessage, Message};

pub async fn create(pool: &PgPool, msg: &CreateMessage) -> Result<Message, sqlx::Error> {
    sqlx::query_as(
        "INSERT INTO messages \
         (inbox_id, thread_id, message_id_header, in_reply_to, references_header, \
          from_addr, to_addrs, cc_addrs, subject, text_body, html_body, \
          extracted_text, direction, raw_headers) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) \
         RETURNING id, inbox_id, thread_id, message_id_header, in_reply_to, \
         references_header, from_addr, to_addrs, cc_addrs, subject, text_body, html_body, \
         extracted_text, direction, raw_headers, created_at",
    )
    .bind(msg.inbox_id)
    .bind(msg.thread_id)
    .bind(&msg.message_id_header)
    .bind(&msg.in_reply_to)
    .bind(&msg.references_header)
    .bind(&msg.from_addr)
    .bind(&msg.to_addrs)
    .bind(&msg.cc_addrs)
    .bind(&msg.subject)
    .bind(&msg.text_body)
    .bind(&msg.html_body)
    .bind(&msg.extracted_text)
    .bind(&msg.direction)
    .bind(&msg.raw_headers)
    .fetch_one(pool)
    .await
}

pub async fn get_by_id(pool: &PgPool, id: Uuid) -> Result<Option<Message>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, inbox_id, thread_id, message_id_header, in_reply_to, \
         references_header, from_addr, to_addrs, cc_addrs, subject, text_body, html_body, \
         extracted_text, direction, raw_headers, created_at \
         FROM messages WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn list_by_inbox(
    pool: &PgPool,
    inbox_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<Message>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, inbox_id, thread_id, message_id_header, in_reply_to, \
         references_header, from_addr, to_addrs, cc_addrs, subject, text_body, html_body, \
         extracted_text, direction, raw_headers, created_at \
         FROM messages WHERE inbox_id = $1 \
         ORDER BY created_at DESC \
         LIMIT $2 OFFSET $3",
    )
    .bind(inbox_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
}

pub async fn list_by_thread(pool: &PgPool, thread_id: Uuid) -> Result<Vec<Message>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, inbox_id, thread_id, message_id_header, in_reply_to, \
         references_header, from_addr, to_addrs, cc_addrs, subject, text_body, html_body, \
         extracted_text, direction, raw_headers, created_at \
         FROM messages WHERE thread_id = $1 \
         ORDER BY created_at ASC",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await
}

pub async fn find_by_message_id_header(
    pool: &PgPool,
    inbox_id: Uuid,
    message_id_header: &str,
) -> Result<Option<Message>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, inbox_id, thread_id, message_id_header, in_reply_to, \
         references_header, from_addr, to_addrs, cc_addrs, subject, text_body, html_body, \
         extracted_text, direction, raw_headers, created_at \
         FROM messages WHERE inbox_id = $1 AND message_id_header = $2",
    )
    .bind(inbox_id)
    .bind(message_id_header)
    .fetch_optional(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Integration tests — require DATABASE_URL with migrations applied

    async fn setup_inbox(pool: &sqlx::PgPool) -> crate::models::Inbox {
        let org = crate::db::organizations::create(pool, "Msg Test Org")
            .await
            .unwrap();
        let email = format!("msg-{}@example.com", Uuid::new_v4());
        crate::db::inboxes::create(pool, org.id, &email, None, "native")
            .await
            .unwrap()
    }

    fn test_create_message(inbox_id: Uuid) -> CreateMessage {
        CreateMessage {
            inbox_id,
            thread_id: None,
            message_id_header: Some(format!("<{}>", Uuid::new_v4())),
            in_reply_to: None,
            references_header: None,
            from_addr: "sender@example.com".into(),
            to_addrs: json!(["rcpt@example.com"]),
            cc_addrs: None,
            subject: Some("Test Subject".into()),
            text_body: Some("Hello".into()),
            html_body: None,
            extracted_text: None,
            direction: "inbound".into(),
            raw_headers: None,
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_message_create_and_get() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let cm = test_create_message(inbox.id);

        let msg = create(&pool, &cm).await.unwrap();
        assert_eq!(msg.inbox_id, inbox.id);
        assert_eq!(msg.from_addr, "sender@example.com");
        assert_eq!(msg.direction, "inbound");
        assert_eq!(msg.to_addrs, json!(["rcpt@example.com"]));

        let fetched = get_by_id(&pool, msg.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, msg.id);
    }

    #[tokio::test]
    #[ignore]
    async fn test_message_list_by_inbox_newest_first() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;

        let mut cm1 = test_create_message(inbox.id);
        cm1.subject = Some("First".into());
        let msg1 = create(&pool, &cm1).await.unwrap();

        let mut cm2 = test_create_message(inbox.id);
        cm2.subject = Some("Second".into());
        let msg2 = create(&pool, &cm2).await.unwrap();

        let msgs = list_by_inbox(&pool, inbox.id, 10, 0).await.unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].id, msg2.id);
        assert_eq!(msgs[1].id, msg1.id);
    }

    #[tokio::test]
    #[ignore]
    async fn test_message_list_by_inbox_pagination() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;

        for _ in 0..3 {
            create(&pool, &test_create_message(inbox.id)).await.unwrap();
        }

        let page = list_by_inbox(&pool, inbox.id, 2, 0).await.unwrap();
        assert_eq!(page.len(), 2);

        let page2 = list_by_inbox(&pool, inbox.id, 2, 2).await.unwrap();
        assert_eq!(page2.len(), 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_message_list_by_thread_chronological() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let thread = crate::db::threads::create(&pool, inbox.id, Some("Thread"))
            .await
            .unwrap();

        let mut cm1 = test_create_message(inbox.id);
        cm1.thread_id = Some(thread.id);
        cm1.subject = Some("First".into());
        let msg1 = create(&pool, &cm1).await.unwrap();

        let mut cm2 = test_create_message(inbox.id);
        cm2.thread_id = Some(thread.id);
        cm2.subject = Some("Second".into());
        let msg2 = create(&pool, &cm2).await.unwrap();

        let msgs = list_by_thread(&pool, thread.id).await.unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].id, msg1.id);
        assert_eq!(msgs[1].id, msg2.id);
    }

    #[tokio::test]
    #[ignore]
    async fn test_message_find_by_message_id_header() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;

        let mut cm = test_create_message(inbox.id);
        let mid = format!("<unique-{}>", Uuid::new_v4());
        cm.message_id_header = Some(mid.clone());

        let msg = create(&pool, &cm).await.unwrap();
        let found = find_by_message_id_header(&pool, inbox.id, &mid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.id, msg.id);
    }

    #[tokio::test]
    #[ignore]
    async fn test_message_find_by_message_id_header_nonexistent() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let result = find_by_message_id_header(&pool, inbox.id, "<nope@nowhere>")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_message_jsonb_fields() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;

        let mut cm = test_create_message(inbox.id);
        cm.to_addrs = json!(["a@b.com", "c@d.com"]);
        cm.cc_addrs = Some(json!(["e@f.com"]));
        cm.raw_headers = Some(json!({"X-Custom": "value"}));

        let msg = create(&pool, &cm).await.unwrap();
        assert_eq!(msg.to_addrs.as_array().unwrap().len(), 2);
        assert_eq!(msg.cc_addrs.as_ref().unwrap().as_array().unwrap().len(), 1);
        assert_eq!(msg.raw_headers.as_ref().unwrap()["X-Custom"], "value");
    }

    #[tokio::test]
    #[ignore]
    async fn test_message_list_by_inbox_empty() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let msgs = list_by_inbox(&pool, inbox.id, 10, 0).await.unwrap();
        assert!(msgs.is_empty());
    }
}
