use sqlx::PgPool;
use uuid::Uuid;

use crate::models::Label;

pub async fn create(
    pool: &PgPool,
    inbox_id: Uuid,
    name: &str,
    color: Option<&str>,
) -> Result<Label, sqlx::Error> {
    sqlx::query_as(
        "INSERT INTO labels (inbox_id, name, color) \
         VALUES ($1, $2, $3) \
         RETURNING id, inbox_id, name, color, created_at",
    )
    .bind(inbox_id)
    .bind(name)
    .bind(color)
    .fetch_one(pool)
    .await
}

pub async fn get_by_id(pool: &PgPool, id: Uuid) -> Result<Option<Label>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, inbox_id, name, color, created_at \
         FROM labels WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn list_by_inbox(pool: &PgPool, inbox_id: Uuid) -> Result<Vec<Label>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, inbox_id, name, color, created_at \
         FROM labels WHERE inbox_id = $1 ORDER BY name",
    )
    .bind(inbox_id)
    .fetch_all(pool)
    .await
}

pub async fn delete(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM labels WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn add_to_message(
    pool: &PgPool,
    message_id: Uuid,
    label_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO message_labels (message_id, label_id) \
         VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(message_id)
    .bind(label_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn remove_from_message(
    pool: &PgPool,
    message_id: Uuid,
    label_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM message_labels WHERE message_id = $1 AND label_id = $2")
        .bind(message_id)
        .bind(label_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn list_for_message(pool: &PgPool, message_id: Uuid) -> Result<Vec<Label>, sqlx::Error> {
    sqlx::query_as(
        "SELECT l.id, l.inbox_id, l.name, l.color, l.created_at \
         FROM labels l \
         JOIN message_labels ml ON ml.label_id = l.id \
         WHERE ml.message_id = $1 \
         ORDER BY l.name",
    )
    .bind(message_id)
    .fetch_all(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn setup_inbox(pool: &PgPool) -> crate::models::Inbox {
        let org = crate::db::organizations::create(pool, "Label Test Org")
            .await
            .unwrap();
        let email = format!("label-{}@example.com", Uuid::new_v4());
        crate::db::inboxes::create(pool, org.id, &email, None, "native")
            .await
            .unwrap()
    }

    async fn setup_message(pool: &PgPool, inbox_id: Uuid) -> crate::models::Message {
        crate::db::messages::create(
            pool,
            &crate::models::CreateMessage {
                inbox_id,
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
                direction: "inbound".into(),
                raw_headers: None,
            },
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    #[ignore]
    async fn test_label_create_and_get() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;

        let label = create(&pool, inbox.id, "important", Some("#ff0000"))
            .await
            .unwrap();
        assert_eq!(label.name, "important");
        assert_eq!(label.color.as_deref(), Some("#ff0000"));
        assert_eq!(label.inbox_id, inbox.id);

        let fetched = get_by_id(&pool, label.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, label.id);
    }

    #[tokio::test]
    #[ignore]
    async fn test_label_duplicate_name_same_inbox_fails() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;

        create(&pool, inbox.id, "dup", None).await.unwrap();
        let err = create(&pool, inbox.id, "dup", None).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    #[ignore]
    async fn test_label_list_by_inbox() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;

        create(&pool, inbox.id, "alpha", None).await.unwrap();
        create(&pool, inbox.id, "beta", None).await.unwrap();

        let labels = list_by_inbox(&pool, inbox.id).await.unwrap();
        assert_eq!(labels.len(), 2);
        assert_eq!(labels[0].name, "alpha");
        assert_eq!(labels[1].name, "beta");
    }

    #[tokio::test]
    #[ignore]
    async fn test_label_list_by_inbox_empty() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let labels = list_by_inbox(&pool, inbox.id).await.unwrap();
        assert!(labels.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_label_delete() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let label = create(&pool, inbox.id, "del", None).await.unwrap();

        assert!(delete(&pool, label.id).await.unwrap());
        assert!(get_by_id(&pool, label.id).await.unwrap().is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_label_delete_nonexistent_returns_false() {
        let pool = crate::db::test_pool().await;
        assert!(!delete(&pool, Uuid::new_v4()).await.unwrap());
    }

    #[tokio::test]
    #[ignore]
    async fn test_label_add_to_message() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let label = create(&pool, inbox.id, "tag", None).await.unwrap();
        let msg = setup_message(&pool, inbox.id).await;

        add_to_message(&pool, msg.id, label.id).await.unwrap();

        let labels = list_for_message(&pool, msg.id).await.unwrap();
        assert_eq!(labels.len(), 1);
        assert_eq!(labels[0].id, label.id);

        // Idempotent — second add should not fail
        add_to_message(&pool, msg.id, label.id).await.unwrap();
        let labels = list_for_message(&pool, msg.id).await.unwrap();
        assert_eq!(labels.len(), 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_label_remove_from_message() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let label = create(&pool, inbox.id, "removable", None).await.unwrap();
        let msg = setup_message(&pool, inbox.id).await;

        add_to_message(&pool, msg.id, label.id).await.unwrap();
        assert!(remove_from_message(&pool, msg.id, label.id).await.unwrap());

        let labels = list_for_message(&pool, msg.id).await.unwrap();
        assert!(labels.is_empty());

        // Removing again returns false
        assert!(!remove_from_message(&pool, msg.id, label.id).await.unwrap());
    }

    #[tokio::test]
    #[ignore]
    async fn test_label_list_for_message() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let l1 = create(&pool, inbox.id, "aaa", None).await.unwrap();
        let l2 = create(&pool, inbox.id, "zzz", None).await.unwrap();
        let msg = setup_message(&pool, inbox.id).await;

        add_to_message(&pool, msg.id, l1.id).await.unwrap();
        add_to_message(&pool, msg.id, l2.id).await.unwrap();

        let labels = list_for_message(&pool, msg.id).await.unwrap();
        assert_eq!(labels.len(), 2);
        assert_eq!(labels[0].name, "aaa");
        assert_eq!(labels[1].name, "zzz");
    }

    #[tokio::test]
    #[ignore]
    async fn test_label_list_for_message_empty() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let msg = setup_message(&pool, inbox.id).await;
        let labels = list_for_message(&pool, msg.id).await.unwrap();
        assert!(labels.is_empty());
    }
}
