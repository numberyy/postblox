//! Full-text search over messages via FTS5.
//!
//! The FTS5 query syntax is exposed verbatim. We wrap quoting so callers
//! can pass user-typed strings without worrying about FTS reserved
//! tokens.

use chrono::{DateTime, Utc};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use crate::db::messages::message_summary_cols;
use crate::db::DbError;
use crate::models::{AccountId, FolderId, MessageSummary, ThreadId};

/// `\Seen` marks a message as read; absence means unread. Stored as a
/// JSON-array entry in `messages.flags`.
const SEEN_FLAG: &str = "\\Seen";

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
) -> Result<Vec<MessageSummary>, DbError> {
    search_scoped(pool, fts_query, None, limit, offset).await
}

#[cfg(test)]
const SEARCH_COLS: &str = message_summary_cols!("m.");

const SEARCH_BY_ACCOUNT_QUERY: &str = concat!(
    "SELECT ",
    message_summary_cols!("m."),
    " FROM messages_fts f JOIN messages m ON m.rowid = f.rowid \
     WHERE messages_fts MATCH ? AND m.account_id = ? \
     ORDER BY rank LIMIT ? OFFSET ?"
);

const SEARCH_ALL_QUERY: &str = concat!(
    "SELECT ",
    message_summary_cols!("m."),
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
    account_id: Option<AccountId>,
    limit: i64,
    offset: i64,
) -> Result<Vec<MessageSummary>, DbError> {
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

/// Optional filters layered on top of the FTS5 MATCH. Every field
/// defaults to `None` (no filter); construct with `..Default::default()`
/// to keep call sites brief.
#[derive(Default, Debug, Clone)]
pub struct SearchFilters {
    /// Limit results to a single account.
    pub account_id: Option<AccountId>,
    /// Limit results to a single folder.
    pub folder_id: Option<FolderId>,
    /// Limit results to a single thread.
    pub thread_id: Option<ThreadId>,
    /// Lower bound (inclusive) on the message date.
    pub date_from: Option<DateTime<Utc>>,
    /// Upper bound (inclusive) on the message date.
    pub date_to: Option<DateTime<Utc>>,
    /// Substring match against `messages.from_addr` (case-insensitive
    /// via SQLite default `LIKE` semantics).
    pub from_addr: Option<String>,
    /// Substring match against `messages.to_addrs` JSON text.
    pub to_addr: Option<String>,
    /// `Some(true)` → only messages with at least one attachment;
    /// `Some(false)` → only messages with no attachments.
    pub has_attachments: Option<bool>,
    /// `Some(true)` → only unread (no `\Seen` flag); `Some(false)` →
    /// only read (has `\Seen`).
    pub unread: Option<bool>,
}

/// Run a filtered FTS search. With an empty `SearchFilters` this is
/// equivalent to [`search`].
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the query or row decode fails.
pub async fn search_filtered(
    pool: &SqlitePool,
    fts_query: &str,
    filters: &SearchFilters,
    limit: i64,
    offset: i64,
) -> Result<Vec<MessageSummary>, DbError> {
    let limit = limit.clamp(1, 500);
    let offset = offset.max(0);

    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new("SELECT ");
    qb.push(message_summary_cols!("m."));
    qb.push(
        " FROM messages_fts f JOIN messages m ON m.rowid = f.rowid \
         WHERE messages_fts MATCH ",
    );
    qb.push_bind(fts_query.to_string());

    if let Some(account_id) = filters.account_id {
        qb.push(" AND m.account_id = ");
        qb.push_bind(account_id);
    }
    if let Some(folder_id) = filters.folder_id {
        qb.push(" AND m.folder_id = ");
        qb.push_bind(folder_id);
    }
    if let Some(thread_id) = filters.thread_id {
        qb.push(" AND m.thread_id = ");
        qb.push_bind(thread_id);
    }
    if let Some(date_from) = filters.date_from {
        qb.push(" AND m.internal_date >= ");
        qb.push_bind(date_from);
    }
    if let Some(date_to) = filters.date_to {
        qb.push(" AND m.internal_date <= ");
        qb.push_bind(date_to);
    }
    if let Some(from_addr) = filters.from_addr.as_deref() {
        qb.push(" AND m.from_addr LIKE ");
        qb.push_bind(like_pattern(from_addr));
        qb.push(" ESCAPE '\\'");
    }
    if let Some(to_addr) = filters.to_addr.as_deref() {
        qb.push(" AND m.to_addrs LIKE ");
        qb.push_bind(like_pattern(to_addr));
        qb.push(" ESCAPE '\\'");
    }
    if let Some(has_attachments) = filters.has_attachments {
        if has_attachments {
            qb.push(" AND EXISTS (SELECT 1 FROM attachments a WHERE a.message_id = m.id)");
        } else {
            qb.push(" AND NOT EXISTS (SELECT 1 FROM attachments a WHERE a.message_id = m.id)");
        }
    }
    if let Some(unread) = filters.unread {
        // `messages.flags` is a JSON array of IMAP flag strings. A
        // message is "seen" iff `\Seen` appears in that array.
        if unread {
            qb.push(" AND NOT EXISTS (SELECT 1 FROM json_each(m.flags) WHERE json_each.value = ");
        } else {
            qb.push(" AND EXISTS (SELECT 1 FROM json_each(m.flags) WHERE json_each.value = ");
        }
        qb.push_bind(SEEN_FLAG);
        qb.push(")");
    }

    qb.push(" ORDER BY rank LIMIT ");
    qb.push_bind(limit);
    qb.push(" OFFSET ");
    qb.push_bind(offset);

    Ok(qb
        .build_query_as::<MessageSummary>()
        .fetch_all(pool)
        .await?)
}

/// Wrap a user-supplied substring as a SQL `LIKE` pattern, escaping the
/// literal wildcards `%` and `_` (and the escape char `\` itself) so
/// callers can't sneak them through via the API.
fn like_pattern(needle: &str) -> String {
    let mut out = String::with_capacity(needle.len() + 2);
    out.push('%');
    for ch in needle.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('%');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    async fn seed() -> (SqlitePool, AccountId, FolderId) {
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
        account_id: AccountId,
        folder_id: FolderId,
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
            to_addrs: crate::models::AddressList::default(),
            cc_addrs: crate::models::AddressList::default(),
            bcc_addrs: crate::models::AddressList::default(),
            reply_to: None,
            subject: Some(subject.into()),
            snippet: Some(body.chars().take(80).collect()),
            text_body: Some(body.into()),
            html_body: None,
            raw_size: 0,
            flags: crate::models::MessageFlags::default(),
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
    async fn test_search_summary_excludes_bodies_and_maps_fields() {
        let (pool, a, f) = seed().await;
        let message = crate::db::messages::create(
            &pool,
            &msg(
                a,
                f,
                1,
                "Quarterly invoice",
                "confidential body",
                "alice@acme.com",
            ),
        )
        .await
        .unwrap();

        let hits = search(&pool, &quote_term("invoice"), 50, 0).await.unwrap();

        assert!(!SEARCH_ALL_QUERY.contains("text_body"));
        assert!(!SEARCH_ALL_QUERY.contains("html_body"));
        assert!(!SEARCH_BY_ACCOUNT_QUERY.contains("text_body"));
        assert!(!SEARCH_BY_ACCOUNT_QUERY.contains("html_body"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, message.id);
        assert_eq!(hits[0].account_id, a);
        assert_eq!(hits[0].folder_id, f);
        assert_eq!(hits[0].uid, 1);
        assert_eq!(hits[0].from_addr, "alice@acme.com");
        assert_eq!(hits[0].subject.as_deref(), Some("Quarterly invoice"));
        assert_eq!(hits[0].snippet.as_deref(), Some("confidential body"));
        assert_eq!(hits[0].created_at, message.created_at);
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
        crate::db::messages::set_flags(
            &pool,
            m.id,
            &crate::models::MessageFlags::from(vec!["\\Flagged"]),
        )
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
                "m.raw_size",
                "m.flags",
                "m.internal_date",
                "m.sent_at",
                "m.created_at",
            ]
        );
        assert!(!cols.contains(&"m.text_body"));
        assert!(!cols.contains(&"m.html_body"));
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

    // ---- search_filtered ---------------------------------------------------

    #[tokio::test]
    async fn test_search_filtered_with_no_filters_matches_search_scoped() {
        // Backward-compat regression: empty filters must produce the same
        // hits as the legacy `search_scoped(account_id=Some(_))` path.
        let (pool, a, f) = seed().await;
        crate::db::messages::create(&pool, &msg(a, f, 1, "regress", "alpha", "x@x.com"))
            .await
            .unwrap();
        crate::db::messages::create(&pool, &msg(a, f, 2, "regress", "beta", "y@x.com"))
            .await
            .unwrap();
        let scoped = search_scoped(&pool, &quote_term("regress"), Some(a), 50, 0)
            .await
            .unwrap();
        let filters = SearchFilters {
            account_id: Some(a),
            ..Default::default()
        };
        let filtered = search_filtered(&pool, &quote_term("regress"), &filters, 50, 0)
            .await
            .unwrap();
        let scoped_ids: Vec<_> = scoped.iter().map(|m| m.id).collect();
        let filtered_ids: Vec<_> = filtered.iter().map(|m| m.id).collect();
        assert_eq!(scoped_ids, filtered_ids);
    }

    #[tokio::test]
    async fn test_search_filtered_by_folder_id_returns_only_that_folder() {
        let (pool, a, f) = seed().await;
        let other_folder = crate::db::folders::create(
            &pool,
            &crate::db::folders::NewFolder {
                account_id: a,
                name: "Archive".into(),
                delimiter: "/".into(),
                role: crate::models::FolderRole::Archive,
                selectable: true,
            },
        )
        .await
        .unwrap();
        crate::db::messages::create(&pool, &msg(a, f, 1, "shared", "in inbox", "x@x.com"))
            .await
            .unwrap();
        crate::db::messages::create(
            &pool,
            &msg(a, other_folder.id, 1, "shared", "in archive", "y@x.com"),
        )
        .await
        .unwrap();

        let filters = SearchFilters {
            folder_id: Some(other_folder.id),
            ..Default::default()
        };
        let hits = search_filtered(&pool, &quote_term("shared"), &filters, 50, 0)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].folder_id, other_folder.id);
    }

    #[tokio::test]
    async fn test_search_filtered_by_thread_id_returns_only_that_thread() {
        let (pool, a, f) = seed().await;
        let thread_a = crate::db::threads::create(&pool, a, None, None)
            .await
            .unwrap();
        let thread_b = crate::db::threads::create(&pool, a, None, None)
            .await
            .unwrap();
        let mut m_a = msg(a, f, 1, "needle", "in A", "x@x.com");
        m_a.thread_id = Some(thread_a.id);
        let mut m_b = msg(a, f, 2, "needle", "in B", "y@x.com");
        m_b.thread_id = Some(thread_b.id);
        crate::db::messages::create(&pool, &m_a).await.unwrap();
        crate::db::messages::create(&pool, &m_b).await.unwrap();

        let filters = SearchFilters {
            thread_id: Some(thread_a.id),
            ..Default::default()
        };
        let hits = search_filtered(&pool, &quote_term("needle"), &filters, 50, 0)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].thread_id, Some(thread_a.id));
    }

    #[tokio::test]
    async fn test_search_filtered_by_date_range_returns_only_messages_in_range() {
        let (pool, a, f) = seed().await;
        let now = Utc::now();
        let mut old = msg(a, f, 1, "report", "old", "x@x.com");
        old.internal_date = now - chrono::Duration::days(10);
        let mut mid = msg(a, f, 2, "report", "mid", "y@x.com");
        mid.internal_date = now - chrono::Duration::days(5);
        let mut fresh = msg(a, f, 3, "report", "fresh", "z@x.com");
        fresh.internal_date = now;
        crate::db::messages::create(&pool, &old).await.unwrap();
        crate::db::messages::create(&pool, &mid).await.unwrap();
        crate::db::messages::create(&pool, &fresh).await.unwrap();

        let filters = SearchFilters {
            date_from: Some(now - chrono::Duration::days(7)),
            date_to: Some(now - chrono::Duration::days(1)),
            ..Default::default()
        };
        let hits = search_filtered(&pool, &quote_term("report"), &filters, 50, 0)
            .await
            .unwrap();
        let uids: Vec<_> = hits.iter().map(|m| m.uid).collect();
        assert_eq!(uids, vec![2]);
    }

    #[tokio::test]
    async fn test_search_filtered_by_from_addr_substring_returns_matching_senders() {
        let (pool, a, f) = seed().await;
        crate::db::messages::create(&pool, &msg(a, f, 1, "ping", "body", "alice@acme.com"))
            .await
            .unwrap();
        crate::db::messages::create(&pool, &msg(a, f, 2, "ping", "body", "bob@other.com"))
            .await
            .unwrap();
        crate::db::messages::create(&pool, &msg(a, f, 3, "ping", "body", "carol@acme.com"))
            .await
            .unwrap();

        let filters = SearchFilters {
            from_addr: Some("acme".into()),
            ..Default::default()
        };
        let hits = search_filtered(&pool, &quote_term("ping"), &filters, 50, 0)
            .await
            .unwrap();
        let mut uids: Vec<_> = hits.iter().map(|m| m.uid).collect();
        uids.sort();
        assert_eq!(uids, vec![1, 3]);
    }

    #[tokio::test]
    async fn test_search_filtered_by_from_addr_escapes_like_wildcards() {
        // A user-supplied `%` must NOT act as a wildcard.
        let (pool, a, f) = seed().await;
        crate::db::messages::create(&pool, &msg(a, f, 1, "ping", "body", "alice@acme.com"))
            .await
            .unwrap();
        let filters = SearchFilters {
            from_addr: Some("%acme%".into()),
            ..Default::default()
        };
        let hits = search_filtered(&pool, &quote_term("ping"), &filters, 50, 0)
            .await
            .unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn test_search_filtered_by_unread_returns_only_unread() {
        let (pool, a, f) = seed().await;
        let read =
            crate::db::messages::create(&pool, &msg(a, f, 1, "tag", "already read", "x@x.com"))
                .await
                .unwrap();
        crate::db::messages::set_flags(
            &pool,
            read.id,
            &crate::models::MessageFlags::from(vec!["\\Seen"]),
        )
        .await
        .unwrap();
        let _unread = crate::db::messages::create(&pool, &msg(a, f, 2, "tag", "fresh", "y@x.com"))
            .await
            .unwrap();

        let filters = SearchFilters {
            unread: Some(true),
            ..Default::default()
        };
        let hits = search_filtered(&pool, &quote_term("tag"), &filters, 50, 0)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].uid, 2);

        let filters = SearchFilters {
            unread: Some(false),
            ..Default::default()
        };
        let hits = search_filtered(&pool, &quote_term("tag"), &filters, 50, 0)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].uid, 1);
    }

    #[tokio::test]
    async fn test_search_filtered_by_has_attachments_true_returns_only_with_attachments() {
        let (pool, a, f) = seed().await;
        let with = crate::db::messages::create(
            &pool,
            &msg(a, f, 1, "report", "with attachment", "x@x.com"),
        )
        .await
        .unwrap();
        let _without =
            crate::db::messages::create(&pool, &msg(a, f, 2, "report", "no attachment", "y@x.com"))
                .await
                .unwrap();
        crate::db::attachments::create(
            &pool,
            &crate::db::attachments::NewAttachment {
                message_id: with.id,
                filename: "doc.pdf".into(),
                content_type: "application/pdf".into(),
                content_id: None,
                size_bytes: 42,
                disposition: crate::models::AttachmentDisposition::Attachment,
                storage_path: "/tmp/doc.pdf".into(),
            },
        )
        .await
        .unwrap();

        let filters = SearchFilters {
            has_attachments: Some(true),
            ..Default::default()
        };
        let hits = search_filtered(&pool, &quote_term("report"), &filters, 50, 0)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].uid, 1);

        let filters = SearchFilters {
            has_attachments: Some(false),
            ..Default::default()
        };
        let hits = search_filtered(&pool, &quote_term("report"), &filters, 50, 0)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].uid, 2);
    }

    #[tokio::test]
    async fn test_search_filtered_by_to_addr_substring_returns_matching_recipients() {
        let (pool, a, f) = seed().await;
        let mut to_acme = msg(a, f, 1, "memo", "body", "x@x.com");
        to_acme.to_addrs = crate::models::AddressList::from(vec!["bob@acme.com"]);
        let mut to_other = msg(a, f, 2, "memo", "body", "x@x.com");
        to_other.to_addrs = crate::models::AddressList::from(vec!["bob@other.com"]);
        crate::db::messages::create(&pool, &to_acme).await.unwrap();
        crate::db::messages::create(&pool, &to_other).await.unwrap();

        let filters = SearchFilters {
            to_addr: Some("acme".into()),
            ..Default::default()
        };
        let hits = search_filtered(&pool, &quote_term("memo"), &filters, 50, 0)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].uid, 1);
    }

    #[test]
    fn test_like_pattern_escapes_special_chars() {
        assert_eq!(like_pattern("plain"), "%plain%");
        assert_eq!(like_pattern("50%"), "%50\\%%");
        assert_eq!(like_pattern("a_b"), "%a\\_b%");
        assert_eq!(like_pattern("c\\d"), "%c\\\\d%");
    }
}
