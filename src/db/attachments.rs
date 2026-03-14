use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{Attachment, CreateAttachment};

pub async fn create(
    pool: &PgPool,
    attachment: &CreateAttachment,
) -> Result<Attachment, sqlx::Error> {
    sqlx::query_as(
        "INSERT INTO attachments (message_id, filename, content_type, size_bytes, storage_key, disposition) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         RETURNING id, message_id, filename, content_type, size_bytes, storage_key, disposition, created_at",
    )
    .bind(attachment.message_id)
    .bind(&attachment.filename)
    .bind(&attachment.content_type)
    .bind(attachment.size_bytes)
    .bind(&attachment.storage_key)
    .bind(&attachment.disposition)
    .fetch_one(pool)
    .await
}

pub async fn list_by_message(
    pool: &PgPool,
    message_id: Uuid,
) -> Result<Vec<Attachment>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, message_id, filename, content_type, size_bytes, storage_key, disposition, created_at \
         FROM attachments WHERE message_id = $1 ORDER BY created_at",
    )
    .bind(message_id)
    .fetch_all(pool)
    .await
}

pub async fn get_by_id(pool: &PgPool, id: Uuid) -> Result<Option<Attachment>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, message_id, filename, content_type, size_bytes, storage_key, disposition, created_at \
         FROM attachments WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn delete(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM attachments WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn setup_message(pool: &PgPool) -> (Uuid, crate::models::Message) {
        let org = crate::db::organizations::create(pool, "Attachment Test Org")
            .await
            .unwrap();
        let email = format!("attach-{}@example.com", Uuid::new_v4());
        let inbox = crate::db::inboxes::create(pool, org.id, &email, None, "native")
            .await
            .unwrap();
        let msg = crate::db::messages::create(
            pool,
            &crate::models::CreateMessage {
                inbox_id: inbox.id,
                thread_id: None,
                message_id_header: Some(format!("<{}>", Uuid::new_v4())),
                in_reply_to: None,
                references_header: None,
                from_addr: "test@example.com".into(),
                to_addrs: json!(["rcpt@example.com"]),
                cc_addrs: None,
                subject: Some("Test".into()),
                text_body: Some("Body".into()),
                html_body: None,
                extracted_text: None,
                direction: crate::models::Direction::Inbound,
                raw_headers: None,
            },
        )
        .await
        .unwrap();
        (inbox.id, msg)
    }

    #[tokio::test]
    #[ignore]
    async fn test_attachment_create_and_get() {
        let pool = crate::db::test_pool().await;
        let (_inbox_id, msg) = setup_message(&pool).await;

        let att = create(
            &pool,
            &CreateAttachment {
                message_id: msg.id,
                filename: "report.pdf".into(),
                content_type: "application/pdf".into(),
                size_bytes: 1024,
                storage_key: format!("{}/report.pdf", msg.id),
                disposition: "attachment".into(),
            },
        )
        .await
        .unwrap();

        assert_eq!(att.filename, "report.pdf");
        assert_eq!(att.content_type, "application/pdf");
        assert_eq!(att.size_bytes, 1024);
        assert_eq!(att.message_id, msg.id);

        let fetched = get_by_id(&pool, att.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, att.id);
        assert_eq!(fetched.filename, "report.pdf");
    }

    #[tokio::test]
    #[ignore]
    async fn test_attachment_list_by_message() {
        let pool = crate::db::test_pool().await;
        let (_inbox_id, msg) = setup_message(&pool).await;

        create(
            &pool,
            &CreateAttachment {
                message_id: msg.id,
                filename: "a.txt".into(),
                content_type: "text/plain".into(),
                size_bytes: 100,
                storage_key: format!("{}/a.txt", msg.id),
                disposition: "attachment".into(),
            },
        )
        .await
        .unwrap();

        create(
            &pool,
            &CreateAttachment {
                message_id: msg.id,
                filename: "b.png".into(),
                content_type: "image/png".into(),
                size_bytes: 2048,
                storage_key: format!("{}/b.png", msg.id),
                disposition: "inline".into(),
            },
        )
        .await
        .unwrap();

        let atts = list_by_message(&pool, msg.id).await.unwrap();
        assert_eq!(atts.len(), 2);
    }

    #[tokio::test]
    #[ignore]
    async fn test_attachment_list_by_message_empty() {
        let pool = crate::db::test_pool().await;
        let (_inbox_id, msg) = setup_message(&pool).await;
        let atts = list_by_message(&pool, msg.id).await.unwrap();
        assert!(atts.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_attachment_delete() {
        let pool = crate::db::test_pool().await;
        let (_inbox_id, msg) = setup_message(&pool).await;

        let att = create(
            &pool,
            &CreateAttachment {
                message_id: msg.id,
                filename: "del.txt".into(),
                content_type: "text/plain".into(),
                size_bytes: 50,
                storage_key: format!("{}/del.txt", msg.id),
                disposition: "attachment".into(),
            },
        )
        .await
        .unwrap();

        assert!(delete(&pool, att.id).await.unwrap());
        assert!(get_by_id(&pool, att.id).await.unwrap().is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_attachment_delete_nonexistent_returns_false() {
        let pool = crate::db::test_pool().await;
        assert!(!delete(&pool, Uuid::new_v4()).await.unwrap());
    }

    #[tokio::test]
    #[ignore]
    async fn test_attachment_cascade_delete_on_message() {
        let pool = crate::db::test_pool().await;
        let (_inbox_id, msg) = setup_message(&pool).await;

        let att = create(
            &pool,
            &CreateAttachment {
                message_id: msg.id,
                filename: "cascade.txt".into(),
                content_type: "text/plain".into(),
                size_bytes: 10,
                storage_key: format!("{}/cascade.txt", msg.id),
                disposition: "attachment".into(),
            },
        )
        .await
        .unwrap();

        sqlx::query("DELETE FROM messages WHERE id = $1")
            .bind(msg.id)
            .execute(&pool)
            .await
            .unwrap();

        assert!(get_by_id(&pool, att.id).await.unwrap().is_none());
    }
}
