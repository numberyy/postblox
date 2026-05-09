use sqlx::SqlitePool;
use uuid::Uuid;

use crate::db::DbError;
use crate::models::{Attachment, AttachmentDisposition};

const COLS: &str = "id, message_id, filename, content_type, content_id, size_bytes, \
    disposition, storage_path, created_at";

#[derive(Debug, Clone)]
pub struct NewAttachment {
    pub message_id: Uuid,
    pub filename: String,
    pub content_type: String,
    pub content_id: Option<String>,
    pub size_bytes: i64,
    pub disposition: AttachmentDisposition,
    pub storage_path: String,
}

pub async fn create(pool: &SqlitePool, new: &NewAttachment) -> Result<Attachment, DbError> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO attachments (id, message_id, filename, content_type, content_id, \
         size_bytes, disposition, storage_path) VALUES (?,?,?,?,?,?,?,?)",
    )
    .bind(id)
    .bind(new.message_id)
    .bind(&new.filename)
    .bind(&new.content_type)
    .bind(&new.content_id)
    .bind(new.size_bytes)
    .bind(new.disposition)
    .bind(&new.storage_path)
    .execute(pool)
    .await?;
    Ok(
        sqlx::query_as::<_, Attachment>(&format!("SELECT {COLS} FROM attachments WHERE id = ?"))
            .bind(id)
            .fetch_one(pool)
            .await?,
    )
}

pub async fn list_for_message(
    pool: &SqlitePool,
    message_id: Uuid,
) -> Result<Vec<Attachment>, DbError> {
    Ok(sqlx::query_as::<_, Attachment>(&format!(
        "SELECT {COLS} FROM attachments WHERE message_id = ? ORDER BY created_at"
    ))
    .bind(message_id)
    .fetch_all(pool)
    .await?)
}

pub async fn get(pool: &SqlitePool, id: Uuid) -> Result<Option<Attachment>, DbError> {
    Ok(
        sqlx::query_as::<_, Attachment>(&format!("SELECT {COLS} FROM attachments WHERE id = ?"))
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{accounts, folders, messages, test_pool};
    use crate::models::{AuthKind, FolderRole};
    use chrono::Utc;
    use serde_json::json;

    async fn message_id_for_test(pool: &SqlitePool) -> Uuid {
        let a = accounts::create(
            pool,
            &accounts::NewAccount {
                email: format!("u-{}@x.com", Uuid::new_v4()),
                display_name: None,
                auth_kind: AuthKind::Password,
                imap_host: "i.x".into(),
                imap_port: 993,
                imap_use_tls: true,
                smtp_host: "s.x".into(),
                smtp_port: 465,
                smtp_use_tls: true,
                smtp_starttls: false,
            },
        )
        .await
        .unwrap();
        let f = folders::upsert(
            pool,
            &folders::NewFolder {
                account_id: a.id,
                name: "INBOX".into(),
                delimiter: "/".into(),
                role: FolderRole::Inbox,
                selectable: true,
            },
        )
        .await
        .unwrap();
        let m = messages::create(
            pool,
            &messages::NewMessage {
                account_id: a.id,
                folder_id: f.id,
                thread_id: None,
                uid: 1,
                message_id_header: Some("<m@x>".into()),
                in_reply_to: None,
                references_header: None,
                from_addr: "alice@x".into(),
                to_addrs: json!([]),
                cc_addrs: json!([]),
                bcc_addrs: json!([]),
                reply_to: None,
                subject: None,
                snippet: None,
                text_body: None,
                html_body: None,
                raw_size: 0,
                flags: json!([]),
                internal_date: Utc::now(),
                sent_at: None,
            },
        )
        .await
        .unwrap();
        m.id
    }

    #[tokio::test]
    async fn test_create_and_list() {
        let pool = test_pool().await;
        let mid = message_id_for_test(&pool).await;
        let a = create(
            &pool,
            &NewAttachment {
                message_id: mid,
                filename: "report.pdf".into(),
                content_type: "application/pdf".into(),
                content_id: None,
                size_bytes: 1234,
                disposition: AttachmentDisposition::Attachment,
                storage_path: "abc/report.pdf".into(),
            },
        )
        .await
        .unwrap();
        let list = list_for_message(&pool, mid).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, a.id);
    }

    #[tokio::test]
    async fn test_list_for_unknown_message_is_empty() {
        let pool = test_pool().await;
        let list = list_for_message(&pool, Uuid::new_v4()).await.unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn test_message_cascade_deletes_attachments() {
        let pool = test_pool().await;
        let mid = message_id_for_test(&pool).await;
        create(
            &pool,
            &NewAttachment {
                message_id: mid,
                filename: "x.bin".into(),
                content_type: "application/octet-stream".into(),
                content_id: None,
                size_bytes: 1,
                disposition: AttachmentDisposition::Attachment,
                storage_path: "x".into(),
            },
        )
        .await
        .unwrap();
        sqlx::query("DELETE FROM messages WHERE id = ?")
            .bind(mid)
            .execute(&pool)
            .await
            .unwrap();
        let list = list_for_message(&pool, mid).await.unwrap();
        assert!(list.is_empty());
    }
}
