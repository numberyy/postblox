use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{CreateDraft, Draft};

pub async fn create(pool: &PgPool, draft: &CreateDraft) -> Result<Draft, sqlx::Error> {
    sqlx::query_as(
        "INSERT INTO drafts (inbox_id, to_addrs, cc_addrs, subject, text_body, html_body, in_reply_to_message_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         RETURNING id, inbox_id, to_addrs, cc_addrs, subject, text_body, html_body, \
         in_reply_to_message_id, updated_at, created_at",
    )
    .bind(draft.inbox_id)
    .bind(&draft.to_addrs)
    .bind(&draft.cc_addrs)
    .bind(&draft.subject)
    .bind(&draft.text_body)
    .bind(&draft.html_body)
    .bind(draft.in_reply_to_message_id)
    .fetch_one(pool)
    .await
}

pub async fn get_by_id(pool: &PgPool, id: Uuid) -> Result<Option<Draft>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, inbox_id, to_addrs, cc_addrs, subject, text_body, html_body, \
         in_reply_to_message_id, updated_at, created_at \
         FROM drafts WHERE id = $1",
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
) -> Result<Vec<Draft>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, inbox_id, to_addrs, cc_addrs, subject, text_body, html_body, \
         in_reply_to_message_id, updated_at, created_at \
         FROM drafts WHERE inbox_id = $1 \
         ORDER BY updated_at DESC \
         LIMIT $2 OFFSET $3",
    )
    .bind(inbox_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
}

pub async fn update(
    pool: &PgPool,
    id: Uuid,
    to_addrs: &serde_json::Value,
    cc_addrs: Option<&serde_json::Value>,
    subject: Option<&str>,
    text_body: Option<&str>,
    html_body: Option<&str>,
) -> Result<Option<Draft>, sqlx::Error> {
    sqlx::query_as(
        "UPDATE drafts SET to_addrs = $2, cc_addrs = $3, subject = $4, \
         text_body = $5, html_body = $6, updated_at = now() \
         WHERE id = $1 \
         RETURNING id, inbox_id, to_addrs, cc_addrs, subject, text_body, html_body, \
         in_reply_to_message_id, updated_at, created_at",
    )
    .bind(id)
    .bind(to_addrs)
    .bind(cc_addrs)
    .bind(subject)
    .bind(text_body)
    .bind(html_body)
    .fetch_optional(pool)
    .await
}

pub async fn delete(pool: &PgPool, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM drafts WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn setup_inbox(pool: &PgPool) -> crate::models::Inbox {
        let org = crate::db::organizations::create(pool, "Draft Test Org")
            .await
            .unwrap();
        let email = format!("draft-{}@example.com", Uuid::new_v4());
        crate::db::inboxes::create(pool, org.id, &email, None, "native")
            .await
            .unwrap()
    }

    fn test_create_draft(inbox_id: Uuid) -> CreateDraft {
        CreateDraft {
            inbox_id,
            to_addrs: json!(["user@example.com"]),
            cc_addrs: None,
            subject: Some("Draft subject".into()),
            text_body: Some("Body".into()),
            html_body: None,
            in_reply_to_message_id: None,
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_draft_create_and_get() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let cd = test_create_draft(inbox.id);

        let draft = create(&pool, &cd).await.unwrap();
        assert_eq!(draft.inbox_id, inbox.id);
        assert_eq!(draft.subject.as_deref(), Some("Draft subject"));

        let fetched = get_by_id(&pool, draft.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, draft.id);
    }

    #[tokio::test]
    #[ignore]
    async fn test_draft_list_by_inbox_newest_first() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;

        let mut cd1 = test_create_draft(inbox.id);
        cd1.subject = Some("First".into());
        let d1 = create(&pool, &cd1).await.unwrap();

        let mut cd2 = test_create_draft(inbox.id);
        cd2.subject = Some("Second".into());
        let d2 = create(&pool, &cd2).await.unwrap();

        let drafts = list_by_inbox(&pool, inbox.id, 10, 0).await.unwrap();
        assert_eq!(drafts.len(), 2);
        assert_eq!(drafts[0].id, d2.id);
        assert_eq!(drafts[1].id, d1.id);
    }

    #[tokio::test]
    #[ignore]
    async fn test_draft_list_by_inbox_empty() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let drafts = list_by_inbox(&pool, inbox.id, 10, 0).await.unwrap();
        assert!(drafts.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_draft_update() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let cd = test_create_draft(inbox.id);
        let draft = create(&pool, &cd).await.unwrap();

        let updated = update(
            &pool,
            draft.id,
            &json!(["new@example.com"]),
            Some(&json!(["cc@example.com"])),
            Some("Updated subject"),
            Some("Updated body"),
            None,
        )
        .await
        .unwrap()
        .unwrap();

        assert_eq!(updated.subject.as_deref(), Some("Updated subject"));
        assert_eq!(updated.to_addrs, json!(["new@example.com"]));
        assert!(updated.updated_at >= draft.updated_at);
    }

    #[tokio::test]
    #[ignore]
    async fn test_draft_update_nonexistent_returns_none() {
        let pool = crate::db::test_pool().await;
        let result = update(&pool, Uuid::new_v4(), &json!([]), None, None, None, None)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_draft_delete() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let draft = create(&pool, &test_create_draft(inbox.id)).await.unwrap();

        assert!(delete(&pool, draft.id).await.unwrap());
        assert!(get_by_id(&pool, draft.id).await.unwrap().is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_draft_delete_nonexistent_returns_false() {
        let pool = crate::db::test_pool().await;
        assert!(!delete(&pool, Uuid::new_v4()).await.unwrap());
    }

    #[tokio::test]
    #[ignore]
    async fn test_draft_pagination() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;

        for _ in 0..3 {
            create(&pool, &test_create_draft(inbox.id)).await.unwrap();
        }

        let page = list_by_inbox(&pool, inbox.id, 2, 0).await.unwrap();
        assert_eq!(page.len(), 2);

        let page2 = list_by_inbox(&pool, inbox.id, 2, 2).await.unwrap();
        assert_eq!(page2.len(), 1);
    }
}
