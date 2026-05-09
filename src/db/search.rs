//! Full-text search over messages via FTS5.
//!
//! The FTS5 query syntax is exposed verbatim. We wrap quoting so callers
//! can pass user-typed strings without worrying about FTS reserved
//! tokens.

use sqlx::SqlitePool;
use uuid::Uuid;

use crate::db::DbError;
use crate::models::Message;

/// Quote a user-supplied search term so FTS5 treats it as a phrase and
/// won't choke on punctuation like `@`, `:`, or `-`.
pub fn quote_term(term: &str) -> String {
    let escaped = term.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

/// Run an FTS5 MATCH against `messages_fts`, joining back to messages.
/// Results are ordered by FTS rank (BM25). Results are unfiltered by
/// account/folder; callers can post-filter with the IDs they care about.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the query or row decode fails — most
/// commonly an FTS5 syntax error in `fts_query` (use [`quote_term`] to
/// neutralise user input), but also any other SQLite error.
pub async fn search(
    pool: &SqlitePool,
    fts_query: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<Message>, DbError> {
    search_scoped(pool, fts_query, None, limit, offset).await
}

// Column list aliased by `m.` so the FTS join projects the underlying
// `messages` row. Hoisted out of `search_scoped` so the two query
// strings can `concat!` it at compile time.
macro_rules! search_cols {
    () => {
        "m.id, m.account_id, m.folder_id, m.thread_id, m.uid, m.message_id_header, \
         m.in_reply_to, m.references_header, m.from_addr, m.to_addrs, m.cc_addrs, \
         m.bcc_addrs, m.reply_to, m.subject, m.snippet, m.text_body, m.html_body, \
         m.raw_size, m.flags, m.internal_date, m.sent_at, m.created_at"
    };
}

#[cfg(test)]
const SEARCH_COLS: &str = search_cols!();

const SEARCH_BY_ACCOUNT_QUERY: &str = concat!(
    "SELECT ",
    search_cols!(),
    " FROM messages_fts f JOIN messages m ON m.rowid = f.rowid \
     WHERE messages_fts MATCH ? AND m.account_id = ? \
     ORDER BY rank LIMIT ? OFFSET ?"
);

const SEARCH_ALL_QUERY: &str = concat!(
    "SELECT ",
    search_cols!(),
    " FROM messages_fts f JOIN messages m ON m.rowid = f.rowid \
     WHERE messages_fts MATCH ? \
     ORDER BY rank LIMIT ? OFFSET ?"
);

/// Like [`search`], but optionally restricts hits to a specific account.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the query or row decode fails — most
/// commonly an FTS5 syntax error in `fts_query` (use [`quote_term`] to
/// neutralise user input), but also any other SQLite error.
pub async fn search_scoped(
    pool: &SqlitePool,
    fts_query: &str,
    account_id: Option<Uuid>,
    limit: i64,
    offset: i64,
) -> Result<Vec<Message>, DbError> {
    let limit = limit.clamp(1, 500);
    let offset = offset.max(0);
    match account_id {
        Some(account_id) => Ok(sqlx::query_as(SEARCH_BY_ACCOUNT_QUERY)
            .bind(fts_query)
            .bind(account_id)
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await?),
        None => Ok(sqlx::query_as(SEARCH_ALL_QUERY)
            .bind(fts_query)
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    async fn seed() -> (SqlitePool, Uuid, Uuid) {
        let pool = crate::db::test_pool().await;
        let acc = crate::db::accounts::create(
            &pool,
            &crate::db::accounts::NewAccount {
                email: "u@x.com".into(),
                display_name: None,
                auth_kind: crate::models::AuthKind::Password,
                imap_host: "i".into(),
                imap_port: 993,
                imap_use_tls: true,
                smtp_host: "s".into(),
                smtp_port: 465,
                smtp_use_tls: true,
                smtp_starttls: false,
            },
        )
        .await
        .unwrap();
        let folder = crate::db::folders::create(
            &pool,
            &crate::db::folders::NewFolder {
                account_id: acc.id,
                name: "INBOX".into(),
                delimiter: "/".into(),
                role: crate::models::FolderRole::Inbox,
                selectable: true,
            },
        )
        .await
        .unwrap();
        (pool, acc.id, folder.id)
    }

    fn msg(
        account_id: Uuid,
        folder_id: Uuid,
        uid: i64,
        subject: &str,
        body: &str,
        from: &str,
    ) -> crate::db::messages::NewMessage {
        crate::db::messages::NewMessage {
            account_id,
            folder_id,
            thread_id: None,
            uid,
            message_id_header: None,
            in_reply_to: None,
            references_header: None,
            from_addr: from.into(),
            to_addrs: json!([]),
            cc_addrs: json!([]),
            bcc_addrs: json!([]),
            reply_to: None,
            subject: Some(subject.into()),
            snippet: Some(body.chars().take(80).collect()),
            text_body: Some(body.into()),
            html_body: None,
            raw_size: 0,
            flags: json!([]),
            internal_date: Utc::now(),
            sent_at: None,
        }
    }

    #[tokio::test]
    async fn test_quote_term_escapes_quotes() {
        // hi "there"  →  "hi ""there"""
        assert_eq!(quote_term(r#"hi "there""#), "\"hi \"\"there\"\"\"");
        assert_eq!(quote_term("plain"), "\"plain\"");
        // Embedded quote round-trip.
        let q = quote_term(r#"a"b"#);
        assert!(q.starts_with('"') && q.ends_with('"'));
        assert!(q.contains(r#"a""b"#));
    }

    #[tokio::test]
    async fn test_search_finds_subject() {
        let (pool, a, f) = seed().await;
        crate::db::messages::create(
            &pool,
            &msg(
                a,
                f,
                1,
                "Quarterly invoice for ACME",
                "details inside",
                "alice@acme.com",
            ),
        )
        .await
        .unwrap();
        crate::db::messages::create(&pool, &msg(a, f, 2, "Lunch?", "are you free", "bob@x.com"))
            .await
            .unwrap();

        let hits = search(&pool, &quote_term("invoice"), 50, 0).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].uid, 1);
    }

    #[tokio::test]
    async fn test_search_finds_body() {
        let (pool, a, f) = seed().await;
        crate::db::messages::create(
            &pool,
            &msg(a, f, 1, "Doc", "the launch is on Friday morning", "x@x.com"),
        )
        .await
        .unwrap();
        let hits = search(&pool, &quote_term("Friday"), 50, 0).await.unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[tokio::test]
    async fn test_search_returns_empty_for_no_match() {
        let (pool, a, f) = seed().await;
        crate::db::messages::create(&pool, &msg(a, f, 1, "Hi", "hello", "x@x.com"))
            .await
            .unwrap();
        let hits = search(&pool, &quote_term("nonexistent"), 50, 0)
            .await
            .unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn test_search_updates_after_message_update() {
        let (pool, a, f) = seed().await;
        let m =
            crate::db::messages::create(&pool, &msg(a, f, 1, "Old subject", "old body", "x@x.com"))
                .await
                .unwrap();
        // Re-index by updating flags (triggers AFTER UPDATE → reindex).
        crate::db::messages::set_flags(&pool, m.id, &json!(["\\Flagged"]))
            .await
            .unwrap();
        let hits = search(&pool, &quote_term("Old"), 50, 0).await.unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[tokio::test]
    async fn test_search_drops_after_message_delete() {
        let (pool, a, f) = seed().await;
        let m = crate::db::messages::create(&pool, &msg(a, f, 1, "deleteable", "body", "x@x.com"))
            .await
            .unwrap();
        assert_eq!(
            search(&pool, &quote_term("deleteable"), 50, 0)
                .await
                .unwrap()
                .len(),
            1
        );
        crate::db::messages::delete(&pool, m.id).await.unwrap();
        assert!(search(&pool, &quote_term("deleteable"), 50, 0)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn test_search_scoped_filters_by_account() {
        let (pool, a, f) = seed().await;
        // Second account in the same DB.
        let other = crate::db::accounts::create(
            &pool,
            &crate::db::accounts::NewAccount {
                email: "other@x.com".into(),
                display_name: None,
                auth_kind: crate::models::AuthKind::Password,
                imap_host: "i".into(),
                imap_port: 993,
                imap_use_tls: true,
                smtp_host: "s".into(),
                smtp_port: 465,
                smtp_use_tls: true,
                smtp_starttls: false,
            },
        )
        .await
        .unwrap();
        let other_folder = crate::db::folders::create(
            &pool,
            &crate::db::folders::NewFolder {
                account_id: other.id,
                name: "INBOX".into(),
                delimiter: "/".into(),
                role: crate::models::FolderRole::Inbox,
                selectable: true,
            },
        )
        .await
        .unwrap();

        crate::db::messages::create(&pool, &msg(a, f, 1, "shared topic", "alpha", "u@x.com"))
            .await
            .unwrap();
        crate::db::messages::create(
            &pool,
            &msg(
                other.id,
                other_folder.id,
                1,
                "shared topic",
                "beta",
                "v@x.com",
            ),
        )
        .await
        .unwrap();

        let all = search_scoped(&pool, &quote_term("shared"), None, 50, 0)
            .await
            .unwrap();
        assert_eq!(all.len(), 2);
        let only_a = search_scoped(&pool, &quote_term("shared"), Some(a), 50, 0)
            .await
            .unwrap();
        assert_eq!(only_a.len(), 1);
        assert_eq!(only_a[0].account_id, a);
    }

    #[tokio::test]
    async fn test_const_search_queries_run_against_empty_index() {
        // Smoke-test both hoisted const queries against an empty FTS
        // index. Catches column-list typos at runtime.
        let (pool, a, _f) = seed().await;
        assert!(search(&pool, &quote_term("anything"), 50, 0)
            .await
            .unwrap()
            .is_empty());
        assert!(
            search_scoped(&pool, &quote_term("anything"), Some(a), 50, 0)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn test_search_cols_const_lists_every_column_aliased() {
        // Snapshot of the aliased column list. Update intentionally.
        let cols: Vec<&str> = SEARCH_COLS.split(',').map(|s| s.trim()).collect();
        assert_eq!(
            cols,
            vec![
                "m.id",
                "m.account_id",
                "m.folder_id",
                "m.thread_id",
                "m.uid",
                "m.message_id_header",
                "m.in_reply_to",
                "m.references_header",
                "m.from_addr",
                "m.to_addrs",
                "m.cc_addrs",
                "m.bcc_addrs",
                "m.reply_to",
                "m.subject",
                "m.snippet",
                "m.text_body",
                "m.html_body",
                "m.raw_size",
                "m.flags",
                "m.internal_date",
                "m.sent_at",
                "m.created_at",
            ]
        );
    }

    #[tokio::test]
    async fn test_search_finds_special_chars_in_quoted_phrase() {
        let (pool, a, f) = seed().await;
        crate::db::messages::create(
            &pool,
            &msg(
                a,
                f,
                1,
                "Hi from alice@example.com",
                "body",
                "alice@example.com",
            ),
        )
        .await
        .unwrap();
        // FTS5 tokenizer splits on @, but quoted phrase still matches.
        let hits = search(&pool, &quote_term("alice@example.com"), 50, 0)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
    }
}
