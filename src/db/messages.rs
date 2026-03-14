use sqlx::PgPool;
use uuid::Uuid;

use crate::models::{CreateMessage, Message, SearchResultWithInbox};

pub const SELECT_COLS: &str = "\
    id, inbox_id, thread_id, message_id_header, in_reply_to, \
    references_header, from_addr, to_addrs, cc_addrs, subject, text_body, html_body, \
    extracted_text, direction, raw_headers, created_at, \
    slop_score, slop_signals, category, priority, triage_status, requires_action";

pub async fn create(pool: &PgPool, msg: &CreateMessage) -> Result<Message, sqlx::Error> {
    let query = format!(
        "INSERT INTO messages \
         (inbox_id, thread_id, message_id_header, in_reply_to, references_header, \
          from_addr, to_addrs, cc_addrs, subject, text_body, html_body, \
          extracted_text, direction, raw_headers) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) \
         RETURNING {SELECT_COLS}"
    );
    sqlx::query_as(&query)
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
        .bind(msg.direction)
        .bind(&msg.raw_headers)
        .fetch_one(pool)
        .await
}

pub async fn get_by_id(pool: &PgPool, id: Uuid) -> Result<Option<Message>, sqlx::Error> {
    let query = format!("SELECT {SELECT_COLS} FROM messages WHERE id = $1");
    sqlx::query_as(&query).bind(id).fetch_optional(pool).await
}

pub async fn list_by_inbox(
    pool: &PgPool,
    inbox_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<Message>, sqlx::Error> {
    let query = format!(
        "SELECT {SELECT_COLS} FROM messages WHERE inbox_id = $1 \
         ORDER BY created_at DESC LIMIT $2 OFFSET $3"
    );
    sqlx::query_as(&query)
        .bind(inbox_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
}

pub async fn list_by_inbox_unslopified(
    pool: &PgPool,
    inbox_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<Message>, sqlx::Error> {
    let query = format!(
        "SELECT {SELECT_COLS} FROM messages WHERE inbox_id = $1 \
         AND (triage_status IS NULL OR triage_status = 'inbox') \
         ORDER BY created_at DESC LIMIT $2 OFFSET $3"
    );
    sqlx::query_as(&query)
        .bind(inbox_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
}

pub async fn list_by_thread(pool: &PgPool, thread_id: Uuid) -> Result<Vec<Message>, sqlx::Error> {
    let query =
        format!("SELECT {SELECT_COLS} FROM messages WHERE thread_id = $1 ORDER BY created_at ASC");
    sqlx::query_as(&query).bind(thread_id).fetch_all(pool).await
}

/// Fetches (thread_id, message_id_header) pairs for an inbox in a single query.
/// Used by inbound pipeline to build ThreadRef list without N+1.
pub async fn message_id_headers_by_inbox(
    pool: &PgPool,
    inbox_id: Uuid,
    limit: i64,
) -> Result<std::collections::HashMap<Uuid, Vec<String>>, sqlx::Error> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT thread_id, message_id_header \
         FROM messages \
         WHERE inbox_id = $1 AND thread_id IS NOT NULL AND message_id_header IS NOT NULL \
         ORDER BY created_at DESC LIMIT $2",
    )
    .bind(inbox_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let mut map: std::collections::HashMap<Uuid, Vec<String>> =
        std::collections::HashMap::with_capacity(rows.len());
    for (thread_id, mid) in rows {
        map.entry(thread_id).or_default().push(mid);
    }
    Ok(map)
}

pub async fn search(
    pool: &PgPool,
    org_id: Uuid,
    query: &str,
    inbox_id: Option<Uuid>,
    limit: i64,
    offset: i64,
) -> Result<Vec<Message>, sqlx::Error> {
    let sql = format!(
        "WITH q AS (SELECT plainto_tsquery('english', $2) AS tsq) \
         SELECT {SELECT_COLS} FROM messages, q \
         WHERE inbox_id IN (SELECT id FROM inboxes WHERE org_id = $1) \
         AND ($5::uuid IS NULL OR inbox_id = $5) \
         AND search_vector @@ q.tsq \
         ORDER BY ts_rank(search_vector, q.tsq) DESC, created_at DESC \
         LIMIT $3 OFFSET $4"
    );
    sqlx::query_as(&sql)
        .bind(org_id)
        .bind(query)
        .bind(limit)
        .bind(offset)
        .bind(inbox_id)
        .fetch_all(pool)
        .await
}

/// Search with joined inbox email — avoids N+1 in dashboard.
pub async fn search_with_inbox(
    pool: &PgPool,
    org_id: Uuid,
    query: &str,
    limit: i64,
) -> Result<Vec<SearchResultWithInbox>, sqlx::Error> {
    sqlx::query_as(
        "WITH q AS (SELECT plainto_tsquery('english', $2) AS tsq) \
         SELECT m.id, m.subject, m.from_addr, m.created_at, i.email AS inbox_email \
         FROM messages m \
         JOIN inboxes i ON i.id = m.inbox_id, q \
         WHERE i.org_id = $1 \
         AND m.search_vector @@ q.tsq \
         ORDER BY ts_rank(m.search_vector, q.tsq) DESC, m.created_at DESC \
         LIMIT $3",
    )
    .bind(org_id)
    .bind(query)
    .bind(limit)
    .fetch_all(pool)
    .await
}

pub async fn find_existing_message_ids(
    pool: &PgPool,
    inbox_id: Uuid,
    message_ids: &[&str],
) -> Result<std::collections::HashSet<String>, sqlx::Error> {
    if message_ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT message_id_header FROM messages \
         WHERE inbox_id = $1 AND message_id_header = ANY($2)",
    )
    .bind(inbox_id)
    .bind(message_ids)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(mid,)| mid).collect())
}

pub async fn find_by_message_id_header(
    pool: &PgPool,
    inbox_id: Uuid,
    message_id_header: &str,
) -> Result<Option<Message>, sqlx::Error> {
    let query = format!(
        "SELECT {SELECT_COLS} FROM messages WHERE inbox_id = $1 AND message_id_header = $2"
    );
    sqlx::query_as(&query)
        .bind(inbox_id)
        .bind(message_id_header)
        .fetch_optional(pool)
        .await
}

pub async fn exists_by_message_id_header(
    pool: &PgPool,
    inbox_id: Uuid,
    message_id_header: &str,
) -> Result<bool, sqlx::Error> {
    let row: (bool,) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM messages WHERE inbox_id = $1 AND message_id_header = $2)",
    )
    .bind(inbox_id)
    .bind(message_id_header)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
            direction: crate::models::Direction::Inbound,
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
        assert_eq!(msg.direction, crate::models::Direction::Inbound);
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
    async fn test_search_finds_matching_message() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let mut cm = test_create_message(inbox.id);
        cm.subject = Some("quarterly revenue report".into());
        cm.text_body = Some("Financial summary for Q4".into());
        create(&pool, &cm).await.unwrap();

        let org_id = crate::db::inboxes::get_by_id(&pool, inbox.id)
            .await
            .unwrap()
            .unwrap()
            .org_id;
        let results = search(&pool, org_id, "revenue", None, 10, 0).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].subject.as_deref(),
            Some("quarterly revenue report")
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_search_no_results() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let cm = test_create_message(inbox.id);
        create(&pool, &cm).await.unwrap();

        let org_id = crate::db::inboxes::get_by_id(&pool, inbox.id)
            .await
            .unwrap()
            .unwrap()
            .org_id;
        let results = search(&pool, org_id, "xyznonexistent", None, 10, 0)
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_search_respects_org_boundary() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let mut cm = test_create_message(inbox.id);
        cm.subject = Some("confidential budget data".into());
        create(&pool, &cm).await.unwrap();

        let other_org = crate::db::organizations::create(&pool, "Other Org")
            .await
            .unwrap();
        let results = search(&pool, other_org.id, "confidential", None, 10, 0)
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_search_pagination() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        for i in 0..3 {
            let mut cm = test_create_message(inbox.id);
            cm.subject = Some(format!("invoice {i}"));
            cm.message_id_header = Some(format!("<search-pg-{i}-{}>", Uuid::new_v4()));
            create(&pool, &cm).await.unwrap();
        }

        let org_id = crate::db::inboxes::get_by_id(&pool, inbox.id)
            .await
            .unwrap()
            .unwrap()
            .org_id;
        let page1 = search(&pool, org_id, "invoice", None, 2, 0).await.unwrap();
        assert_eq!(page1.len(), 2);

        let page2 = search(&pool, org_id, "invoice", None, 2, 2).await.unwrap();
        assert_eq!(page2.len(), 1);
    }

    #[tokio::test]
    #[ignore]
    async fn test_message_list_by_inbox_empty() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let msgs = list_by_inbox(&pool, inbox.id, 10, 0).await.unwrap();
        assert!(msgs.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_cross_inbox_isolation_get() {
        let pool = crate::db::test_pool().await;
        let inbox_a = setup_inbox(&pool).await;
        let inbox_b = setup_inbox(&pool).await;

        let cm = test_create_message(inbox_a.id);
        let msg = create(&pool, &cm).await.unwrap();

        // Message should be found in inbox_a
        let found = get_by_id(&pool, msg.id).await.unwrap().unwrap();
        assert_eq!(found.inbox_id, inbox_a.id);

        // Message should NOT appear in inbox_b's list
        let b_msgs = list_by_inbox(&pool, inbox_b.id, 100, 0).await.unwrap();
        assert!(
            b_msgs.iter().all(|m| m.inbox_id == inbox_b.id),
            "inbox_b list should not contain inbox_a messages"
        );
    }

    #[tokio::test]
    #[ignore]
    async fn test_cross_inbox_isolation_list() {
        let pool = crate::db::test_pool().await;
        let inbox_a = setup_inbox(&pool).await;
        let inbox_b = setup_inbox(&pool).await;

        // Create messages in both inboxes
        for _ in 0..3 {
            let mut cm = test_create_message(inbox_a.id);
            cm.message_id_header = Some(format!("<{}>", Uuid::new_v4()));
            create(&pool, &cm).await.unwrap();
        }
        for _ in 0..2 {
            let mut cm = test_create_message(inbox_b.id);
            cm.message_id_header = Some(format!("<{}>", Uuid::new_v4()));
            create(&pool, &cm).await.unwrap();
        }

        let a_msgs = list_by_inbox(&pool, inbox_a.id, 100, 0).await.unwrap();
        let b_msgs = list_by_inbox(&pool, inbox_b.id, 100, 0).await.unwrap();
        assert!(a_msgs.len() >= 3);
        assert!(b_msgs.len() >= 2);
        assert!(a_msgs.iter().all(|m| m.inbox_id == inbox_a.id));
        assert!(b_msgs.iter().all(|m| m.inbox_id == inbox_b.id));
    }

    #[tokio::test]
    #[ignore]
    async fn test_find_by_message_id_header_dedup() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let mid = format!("dedup-{}@example.com", Uuid::new_v4());

        let mut cm = test_create_message(inbox.id);
        cm.message_id_header = Some(mid.clone());
        create(&pool, &cm).await.unwrap();

        // Should find the existing message
        let found = find_by_message_id_header(&pool, inbox.id, &mid)
            .await
            .unwrap();
        assert!(found.is_some());

        // Different inbox, same message_id — should NOT find it
        let inbox_b = setup_inbox(&pool).await;
        let not_found = find_by_message_id_header(&pool, inbox_b.id, &mid)
            .await
            .unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_message_without_message_id_header() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;

        let mut cm = test_create_message(inbox.id);
        cm.message_id_header = None;
        let msg = create(&pool, &cm).await.unwrap();

        assert!(msg.message_id_header.is_none());
        let fetched = get_by_id(&pool, msg.id).await.unwrap().unwrap();
        assert!(fetched.message_id_header.is_none());
    }

    #[tokio::test]
    #[ignore]
    async fn test_message_id_headers_by_inbox_groups_by_thread() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;
        let thread = crate::db::threads::create(&pool, inbox.id, Some("MID Test"))
            .await
            .unwrap();

        let mid1 = format!("<mid1-{}>", Uuid::new_v4());
        let mid2 = format!("<mid2-{}>", Uuid::new_v4());

        let mut cm1 = test_create_message(inbox.id);
        cm1.thread_id = Some(thread.id);
        cm1.message_id_header = Some(mid1.clone());
        create(&pool, &cm1).await.unwrap();

        let mut cm2 = test_create_message(inbox.id);
        cm2.thread_id = Some(thread.id);
        cm2.message_id_header = Some(mid2.clone());
        create(&pool, &cm2).await.unwrap();

        let map = message_id_headers_by_inbox(&pool, inbox.id, 100)
            .await
            .unwrap();
        let ids = map.get(&thread.id).unwrap();
        assert!(ids.contains(&mid1));
        assert!(ids.contains(&mid2));
    }

    #[tokio::test]
    #[ignore]
    async fn test_message_id_headers_by_inbox_skips_null_thread() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;

        // Message with no thread — should be excluded
        let mut cm = test_create_message(inbox.id);
        cm.thread_id = None;
        cm.message_id_header = Some(format!("<no-thread-{}>", Uuid::new_v4()));
        create(&pool, &cm).await.unwrap();

        let map = message_id_headers_by_inbox(&pool, inbox.id, 100)
            .await
            .unwrap();
        assert!(map.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_find_existing_message_ids_returns_matches() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;

        let mid1 = format!("<existing-{}>", Uuid::new_v4());
        let mid2 = format!("<existing-{}>", Uuid::new_v4());
        let mid_missing = format!("<missing-{}>", Uuid::new_v4());

        let mut cm1 = test_create_message(inbox.id);
        cm1.message_id_header = Some(mid1.clone());
        create(&pool, &cm1).await.unwrap();

        let mut cm2 = test_create_message(inbox.id);
        cm2.message_id_header = Some(mid2.clone());
        create(&pool, &cm2).await.unwrap();

        let existing = find_existing_message_ids(
            &pool,
            inbox.id,
            &[mid1.as_str(), mid2.as_str(), mid_missing.as_str()],
        )
        .await
        .unwrap();

        assert!(existing.contains(&mid1));
        assert!(existing.contains(&mid2));
        assert!(!existing.contains(&mid_missing));
    }

    #[tokio::test]
    #[ignore]
    async fn test_find_existing_message_ids_empty_input() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;

        let result = find_existing_message_ids(&pool, inbox.id, &[])
            .await
            .unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn test_list_by_inbox_unslopified_filters_slopified() {
        let pool = crate::db::test_pool().await;
        let inbox = setup_inbox(&pool).await;

        // Create a normal message (triage_status NULL → should appear)
        let cm1 = test_create_message(inbox.id);
        create(&pool, &cm1).await.unwrap();

        // Create a message and set its triage_status to 'slopified'
        let mut cm2 = test_create_message(inbox.id);
        cm2.message_id_header = Some(format!("<slop-{}>", Uuid::new_v4()));
        let slop_msg = create(&pool, &cm2).await.unwrap();
        sqlx::query("UPDATE messages SET triage_status = 'slopified' WHERE id = $1")
            .bind(slop_msg.id)
            .execute(&pool)
            .await
            .unwrap();

        // Create a message with triage_status = 'inbox' → should appear
        let mut cm3 = test_create_message(inbox.id);
        cm3.message_id_header = Some(format!("<inbox-{}>", Uuid::new_v4()));
        let inbox_msg = create(&pool, &cm3).await.unwrap();
        sqlx::query("UPDATE messages SET triage_status = 'inbox' WHERE id = $1")
            .bind(inbox_msg.id)
            .execute(&pool)
            .await
            .unwrap();

        let results = list_by_inbox_unslopified(&pool, inbox.id, 100, 0)
            .await
            .unwrap();
        // Should include the NULL and 'inbox' messages, exclude 'slopified'
        assert!(results
            .iter()
            .all(|m| m.triage_status.as_deref() != Some("slopified")));
        assert!(results.len() >= 2);
    }
}
