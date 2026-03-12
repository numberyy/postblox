use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::models::Thread;

#[allow(dead_code)]
pub async fn create(
    pool: &PgPool,
    inbox_id: Uuid,
    subject: Option<&str>,
) -> Result<Thread, sqlx::Error> {
    sqlx::query_as(
        "INSERT INTO threads (inbox_id, subject) VALUES ($1, $2) \
         RETURNING id, inbox_id, subject, message_count, last_message_at, created_at",
    )
    .bind(inbox_id)
    .bind(subject)
    .fetch_one(pool)
    .await
}

pub async fn create_with_message(
    pool: &PgPool,
    inbox_id: Uuid,
    subject: Option<&str>,
    message_at: DateTime<Utc>,
) -> Result<Thread, sqlx::Error> {
    sqlx::query_as(
        "INSERT INTO threads (inbox_id, subject, message_count, last_message_at) VALUES ($1, $2, 1, $3) \
         RETURNING id, inbox_id, subject, message_count, last_message_at, created_at",
    )
    .bind(inbox_id)
    .bind(subject)
    .bind(message_at)
    .fetch_one(pool)
    .await
}

pub async fn get_by_id(pool: &PgPool, id: Uuid) -> Result<Option<Thread>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, inbox_id, subject, message_count, last_message_at, created_at \
         FROM threads WHERE id = $1",
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
) -> Result<Vec<Thread>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, inbox_id, subject, message_count, last_message_at, created_at \
         FROM threads WHERE inbox_id = $1 \
         ORDER BY last_message_at DESC NULLS LAST \
         LIMIT $2 OFFSET $3",
    )
    .bind(inbox_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
}

pub async fn increment_message_count(
    pool: &PgPool,
    id: Uuid,
    last_message_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE threads SET message_count = message_count + 1, last_message_at = $2 WHERE id = $1",
    )
    .bind(id)
    .bind(last_message_at)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_inbox(pool: &sqlx::PgPool) -> crate::models::Inbox {
        let org = crate::db::organizations::create(pool, "Thread Test Org")
            .await
            .unwrap();
        let email = format!("thread-{}@example.com", Uuid::new_v4());
        crate::db::inboxes::create(pool, org.id, &email, None, "native")
            .await
            .unwrap()
    }

    #[tokio::test]
    #[ignore]
    async fn test_thread_create_defaults() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;

        let thread = create(&pool, inbox.id, Some("Hello")).await.unwrap();
        assert_eq!(thread.subject.as_deref(), Some("Hello"));
        assert_eq!(thread.message_count, 0);
        assert!(thread.last_message_at.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_thread_create_null_subject() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;

        let thread = create(&pool, inbox.id, None).await.unwrap();
        assert!(thread.subject.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_thread_get_nonexistent() {
        let pool = crate::db::test_pool().await;
        let result = get_by_id(&pool, Uuid::new_v4()).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_thread_list_by_inbox_paginated() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;

        for i in 0..5 {
            let t = create(&pool, inbox.id, Some(&format!("Thread {i}")))
                .await
                .unwrap();
            increment_message_count(&pool, t.id, Utc::now())
                .await
                .unwrap();
        }

        let page1 = list_by_inbox(&pool, inbox.id, 2, 0).await.unwrap();
        assert_eq!(page1.len(), 2);

        let page2 = list_by_inbox(&pool, inbox.id, 2, 2).await.unwrap();
        assert_eq!(page2.len(), 2);

        let page3 = list_by_inbox(&pool, inbox.id, 2, 4).await.unwrap();
        assert_eq!(page3.len(), 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_thread_list_by_inbox_empty() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let threads = list_by_inbox(&pool, inbox.id, 10, 0).await.unwrap();
        assert!(threads.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_thread_list_offset_beyond_results() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        create(&pool, inbox.id, Some("Only")).await.unwrap();
        let threads = list_by_inbox(&pool, inbox.id, 10, 100).await.unwrap();
        assert!(threads.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_thread_increment_message_count() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let thread = create(&pool, inbox.id, Some("Count Test")).await.unwrap();

        let ts1 = Utc::now();
        increment_message_count(&pool, thread.id, ts1)
            .await
            .unwrap();
        increment_message_count(&pool, thread.id, ts1)
            .await
            .unwrap();
        increment_message_count(&pool, thread.id, ts1)
            .await
            .unwrap();

        let updated = get_by_id(&pool, thread.id).await.unwrap().unwrap();
        assert_eq!(updated.message_count, 3);
        assert!(updated.last_message_at.is_some());
    }
}
