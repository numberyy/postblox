//! CRUD for the `messages` table.
//!
//! Holds the canonical row for every IMAP message the daemon has
//! seen: headers, snippet, body parts, flags JSON, and the IMAP UID
//! that lets the sync layer reconcile remote state. Address lists and
//! flags are stored as `serde_json::Value` columns so the daemon can
//! round-trip MIME shapes without a sidecar table. The shared
//! projection macros keep full and summary `SELECT` lists in one place;
//! all access is via the daemon's [`SqlitePool`].

use chrono::{DateTime, Utc};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use crate::db::DbError;
use crate::models::{AccountId, FolderId, Message, MessageId, MessageSummary, ThreadId};

#[derive(Debug, Clone)]
pub struct NewMessage {
    pub account_id: AccountId,
    pub folder_id: FolderId,
    pub thread_id: Option<ThreadId>,
    pub uid: i64,
    pub message_id_header: Option<String>,
    pub in_reply_to: Option<String>,
    pub references_header: Option<String>,
    pub from_addr: String,
    pub to_addrs: serde_json::Value,
    pub cc_addrs: serde_json::Value,
    pub bcc_addrs: serde_json::Value,
    pub reply_to: Option<String>,
    pub subject: Option<String>,
    pub snippet: Option<String>,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
    pub raw_size: i64,
    pub flags: serde_json::Value,
    pub internal_date: DateTime<Utc>,
    pub sent_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetFlagsOutcome {
    pub found: bool,
    pub changed: bool,
}

// Column list shared by every `SELECT` against `messages`. Defined as a
// `macro_rules!` so each query string can `concat!` it at compile time
// instead of `format!`-ing the same prefix on every call. The optional
// prefix arm lets joined queries project `m.id, m.account_id, ...`
// without maintaining a second column list.
macro_rules! message_cols {
    (@columns $mode:ident $($prefix:literal)?) => {
        message_cols!(
            @$mode
            $($prefix,)?
            id,
            account_id,
            folder_id,
            thread_id,
            uid,
            message_id_header,
            in_reply_to,
            references_header,
            from_addr,
            to_addrs,
            cc_addrs,
            bcc_addrs,
            reply_to,
            subject,
            snippet,
            text_body,
            html_body,
            raw_size,
            flags,
            internal_date,
            sent_at,
            created_at,
        )
    };
    (@unaliased $first:ident $(, $rest:ident)* $(,)?) => {
        concat!(stringify!($first) $(, ", ", stringify!($rest))*)
    };
    (@aliased $prefix:literal, $first:ident $(, $rest:ident)* $(,)?) => {
        concat!($prefix, stringify!($first) $(, ", ", $prefix, stringify!($rest))*)
    };
    () => {
        message_cols!(@columns unaliased)
    };
    ($prefix:literal) => {
        message_cols!(@columns aliased $prefix)
    };
}

macro_rules! message_summary_cols {
    (@columns $mode:ident $($prefix:literal)?) => {
        message_summary_cols!(
            @$mode
            $($prefix,)?
            id,
            account_id,
            folder_id,
            thread_id,
            uid,
            message_id_header,
            in_reply_to,
            references_header,
            from_addr,
            to_addrs,
            cc_addrs,
            bcc_addrs,
            reply_to,
            subject,
            snippet,
            raw_size,
            flags,
            internal_date,
            sent_at,
            created_at,
        )
    };
    (@unaliased $first:ident $(, $rest:ident)* $(,)?) => {
        concat!(stringify!($first) $(, ", ", stringify!($rest))*)
    };
    (@aliased $prefix:literal, $first:ident $(, $rest:ident)* $(,)?) => {
        concat!($prefix, stringify!($first) $(, ", ", $prefix, stringify!($rest))*)
    };
    () => {
        message_summary_cols!(@columns unaliased)
    };
    ($prefix:literal) => {
        message_summary_cols!(@columns aliased $prefix)
    };
}
pub(crate) use message_summary_cols;

#[cfg(test)]
const MESSAGE_COLS: &str = message_cols!();
#[cfg(test)]
const MESSAGE_COLS_ALIASED_M: &str = message_cols!("m.");
#[cfg(test)]
const MESSAGE_SUMMARY_COLS: &str = message_summary_cols!();
#[cfg(test)]
const MESSAGE_SUMMARY_COLS_ALIASED_M: &str = message_summary_cols!("m.");

const INSERT_RETURNING_QUERY: &str = concat!(
    "INSERT INTO messages \
     (id, account_id, folder_id, thread_id, uid, message_id_header, in_reply_to, \
      references_header, from_addr, to_addrs, cc_addrs, bcc_addrs, reply_to, \
      subject, snippet, text_body, html_body, raw_size, flags, internal_date, sent_at) \
      VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) RETURNING ",
    message_cols!()
);

const GET_BY_ID_QUERY: &str = concat!("SELECT ", message_cols!(), " FROM messages WHERE id = ?");

const GET_BY_FOLDER_UID_QUERY: &str = concat!(
    "SELECT ",
    message_cols!(),
    " FROM messages WHERE folder_id = ? AND uid = ?"
);

const LIST_BY_FOLDER_QUERY: &str = concat!(
    "SELECT ",
    message_summary_cols!(),
    " FROM messages WHERE folder_id = ? \
     ORDER BY internal_date DESC LIMIT ? OFFSET ?"
);

const LIST_BY_THREAD_QUERY: &str = concat!(
    "SELECT ",
    message_summary_cols!(),
    " FROM messages WHERE thread_id = ? ORDER BY internal_date"
);

const GET_BY_MESSAGE_ID_HEADER_QUERY: &str = concat!(
    "SELECT ",
    message_cols!(),
    " FROM messages \
     WHERE account_id = ? AND message_id_header = ? LIMIT 1"
);

const EXISTING_UIDS_PREFIX: &str = "SELECT uid FROM messages WHERE folder_id = ? AND uid IN (";
const EXISTING_UIDS_SUFFIX: &str = ")";
const SET_FLAGS_QUERY: &str = "UPDATE messages SET flags = ? WHERE id = ? AND flags <> ?";
const MESSAGE_EXISTS_QUERY: &str = "SELECT 1 FROM messages WHERE id = ? LIMIT 1";

/// Insert a message row and return the persisted record. The FTS5
/// trigger reindexes the message synchronously.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the insert fails — typically a `UNIQUE`
/// violation on `(folder_id, uid)`, a FK violation when `account_id` /
/// `folder_id` / `thread_id` is unknown, or any other SQLite error.
pub async fn create(pool: &SqlitePool, new: &NewMessage) -> Result<Message, DbError> {
    let id = MessageId::new();
    Ok(sqlx::query_as(INSERT_RETURNING_QUERY)
        .bind(id)
        .bind(new.account_id)
        .bind(new.folder_id)
        .bind(new.thread_id)
        .bind(new.uid)
        .bind(&new.message_id_header)
        .bind(&new.in_reply_to)
        .bind(&new.references_header)
        .bind(&new.from_addr)
        .bind(&new.to_addrs)
        .bind(&new.cc_addrs)
        .bind(&new.bcc_addrs)
        .bind(&new.reply_to)
        .bind(&new.subject)
        .bind(&new.snippet)
        .bind(&new.text_body)
        .bind(&new.html_body)
        .bind(new.raw_size)
        .bind(&new.flags)
        .bind(new.internal_date)
        .bind(new.sent_at)
        .fetch_one(pool)
        .await?)
}

/// Look up a message by id; `Ok(None)` if missing.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the query or row decode fails. A missing
/// row is reported as `Ok(None)`, not an error.
pub async fn get(pool: &SqlitePool, id: MessageId) -> Result<Option<Message>, DbError> {
    Ok(sqlx::query_as(GET_BY_ID_QUERY)
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// Look up a message by `(folder_id, uid)`; `Ok(None)` if missing.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the query or row decode fails. A missing
/// row is reported as `Ok(None)`, not an error.
pub async fn get_by_folder_uid(
    pool: &SqlitePool,
    folder_id: FolderId,
    uid: i64,
) -> Result<Option<Message>, DbError> {
    Ok(sqlx::query_as(GET_BY_FOLDER_UID_QUERY)
        .bind(folder_id)
        .bind(uid)
        .fetch_optional(pool)
        .await?)
}

/// List messages in a folder, newest first, with `limit` clamped to
/// `[1, 500]` and `offset` clamped to `>= 0`.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the query or row decode fails.
pub async fn list_by_folder(
    pool: &SqlitePool,
    folder_id: FolderId,
    limit: i64,
    offset: i64,
) -> Result<Vec<MessageSummary>, DbError> {
    Ok(sqlx::query_as(LIST_BY_FOLDER_QUERY)
        .bind(folder_id)
        .bind(limit.clamp(1, 500))
        .bind(offset.max(0))
        .fetch_all(pool)
        .await?)
}

/// List the messages in a thread, oldest first.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the query or row decode fails.
pub async fn list_by_thread(
    pool: &SqlitePool,
    thread_id: ThreadId,
) -> Result<Vec<MessageSummary>, DbError> {
    Ok(sqlx::query_as(LIST_BY_THREAD_QUERY)
        .bind(thread_id)
        .fetch_all(pool)
        .await?)
}

/// Return non-null RFC822 Message-ID headers for the supplied threads.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the dynamic-`IN` query or row decode
/// fails. An empty `thread_ids` slice short-circuits to `Ok(empty)`
/// without touching the database.
pub async fn message_ids_by_threads(
    pool: &SqlitePool,
    thread_ids: &[ThreadId],
) -> Result<Vec<(ThreadId, String)>, DbError> {
    if thread_ids.is_empty() {
        return Ok(Vec::new());
    }

    let mut qb: QueryBuilder<Sqlite> =
        QueryBuilder::new("SELECT thread_id, message_id_header FROM messages WHERE thread_id IN (");
    {
        let mut separated = qb.separated(",");
        for thread_id in thread_ids {
            separated.push_bind(*thread_id);
        }
        separated.push_unseparated(
            ") AND message_id_header IS NOT NULL ORDER BY thread_id, internal_date",
        );
    }

    Ok(qb
        .build_query_as::<(ThreadId, String)>()
        .fetch_all(pool)
        .await?)
}

/// Look up a message by its RFC822 Message-ID header within an account. Used
/// by the threading matcher to walk In-Reply-To / References chains.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the query or row decode fails. A missing
/// header / unknown account is reported as `Ok(None)`, not an error.
pub async fn get_by_message_id_header(
    pool: &SqlitePool,
    account_id: AccountId,
    message_id_header: &str,
) -> Result<Option<Message>, DbError> {
    Ok(sqlx::query_as(GET_BY_MESSAGE_ID_HEADER_QUERY)
        .bind(account_id)
        .bind(message_id_header)
        .fetch_optional(pool)
        .await?)
}

/// Return the subset of `uids` that already exist in `folder_id`. Used by
/// IMAP sync to skip messages we've already fetched.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the dynamic-`IN` query or row decode
/// fails. An empty `uids` slice short-circuits to `Ok(empty)` without
/// touching the database.
pub async fn existing_uids(
    pool: &SqlitePool,
    folder_id: FolderId,
    uids: &[i64],
) -> Result<std::collections::HashSet<i64>, DbError> {
    if uids.is_empty() {
        return Ok(Default::default());
    }
    // Each placeholder contributes "?" plus a separating "," — 2 bytes per
    // uid except the last one. Pre-allocate so the loop never re-grows.
    let placeholder_len = uids.len() * 2 - 1;
    let mut q = String::with_capacity(
        EXISTING_UIDS_PREFIX.len() + placeholder_len + EXISTING_UIDS_SUFFIX.len(),
    );
    q.push_str(EXISTING_UIDS_PREFIX);
    for i in 0..uids.len() {
        if i > 0 {
            q.push(',');
        }
        q.push('?');
    }
    q.push_str(EXISTING_UIDS_SUFFIX);
    let mut query = sqlx::query_as::<_, (i64,)>(&q).bind(folder_id);
    for u in uids {
        query = query.bind(u);
    }
    let rows = query.fetch_all(pool).await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

/// Reassign a message to a different thread (or clear with `None`).
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the update fails (FK violation when
/// `thread_id` is unknown, or any other SQLite error). A missing `id`
/// is a silent no-op.
pub async fn set_thread(
    pool: &SqlitePool,
    id: MessageId,
    thread_id: Option<ThreadId>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE messages SET thread_id = ? WHERE id = ?")
        .bind(thread_id)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Replace the flag list for a message. Reports whether a row was
/// found and whether the stored flags actually changed. Only changed
/// rows are updated, avoiding the `AFTER UPDATE` FTS reindex trigger
/// for no-op flag writes.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the update or existence check fails. A
/// missing id is reported as `found: false`, not an error.
pub async fn set_flags(
    pool: &SqlitePool,
    id: MessageId,
    flags: &serde_json::Value,
) -> Result<SetFlagsOutcome, DbError> {
    let r = sqlx::query(SET_FLAGS_QUERY)
        .bind(flags)
        .bind(id)
        .bind(flags)
        .execute(pool)
        .await?;
    if r.rows_affected() > 0 {
        return Ok(SetFlagsOutcome {
            found: true,
            changed: true,
        });
    }
    let found = sqlx::query_scalar::<_, i64>(MESSAGE_EXISTS_QUERY)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .is_some();
    Ok(SetFlagsOutcome {
        found,
        changed: false,
    })
}

/// Reassign a message to a different folder. Returns true when a row
/// matched. Used by archive / move ops; on the wire IMAP this is what a
/// MOVE would do server-side, but here we reflect the change locally
/// and let the next IMAP sync reconcile.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the update fails (FK violation when
/// `folder_id` is unknown, or any other SQLite error). A missing `id`
/// is reported as `Ok(false)`, not an error.
pub async fn set_folder(
    pool: &SqlitePool,
    id: MessageId,
    folder_id: FolderId,
) -> Result<bool, DbError> {
    let r = sqlx::query("UPDATE messages SET folder_id = ? WHERE id = ?")
        .bind(folder_id)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}

/// Delete a message by id. Returns `true` if a row was removed.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the delete fails. A missing row is
/// reported as `Ok(false)`, not an error.
pub async fn delete(pool: &SqlitePool, id: MessageId) -> Result<bool, DbError> {
    let r = sqlx::query("DELETE FROM messages WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}

/// Delete a message identified by `(folder_id, uid)`. Returns `true` if
/// a row was removed.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the delete fails. A missing row is
/// reported as `Ok(false)`, not an error.
pub async fn delete_by_folder_uid(
    pool: &SqlitePool,
    folder_id: FolderId,
    uid: i64,
) -> Result<bool, DbError> {
    let r = sqlx::query("DELETE FROM messages WHERE folder_id = ? AND uid = ?")
        .bind(folder_id)
        .bind(uid)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}

/// Wipe every message in a folder. Used when the server's
/// `UIDVALIDITY` changed under us and we have to refetch from scratch.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the delete fails. Unknown `folder_id`
/// is reported as `Ok(0)`, not an error.
pub async fn delete_all_in_folder(pool: &SqlitePool, folder_id: FolderId) -> Result<u64, DbError> {
    let r = sqlx::query("DELETE FROM messages WHERE folder_id = ?")
        .bind(folder_id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    struct Ctx {
        pool: SqlitePool,
        account_id: AccountId,
        folder_id: FolderId,
        thread_id: ThreadId,
    }

    async fn ctx() -> Ctx {
        let pool = crate::db::test_pool().await;
        let acc = crate::db::accounts::create(
            &pool,
            &crate::db::accounts::NewAccount {
                email: format!("u-{}@x.com", Uuid::new_v4()),
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
        let thread = crate::db::threads::create(&pool, acc.id, None, None)
            .await
            .unwrap();
        Ctx {
            pool,
            account_id: acc.id,
            folder_id: folder.id,
            thread_id: thread.id,
        }
    }

    fn sample(c: &Ctx, uid: i64) -> NewMessage {
        NewMessage {
            account_id: c.account_id,
            folder_id: c.folder_id,
            thread_id: Some(c.thread_id),
            uid,
            message_id_header: Some(format!("<{uid}@x>")),
            in_reply_to: None,
            references_header: None,
            from_addr: "alice@x.com".into(),
            to_addrs: json!(["bob@x.com"]),
            cc_addrs: json!([]),
            bcc_addrs: json!([]),
            reply_to: None,
            subject: Some(format!("subject {uid}")),
            snippet: Some("hi".into()),
            text_body: Some("body".into()),
            html_body: None,
            raw_size: 1234,
            flags: json!(["\\Seen"]),
            internal_date: Utc::now(),
            sent_at: None,
        }
    }

    #[tokio::test]
    async fn test_create_and_get() {
        let c = ctx().await;
        let m = create(&c.pool, &sample(&c, 1)).await.unwrap();
        assert_eq!(m.uid, 1);
        assert_eq!(m.from_addr, "alice@x.com");
        assert_eq!(m.flags, json!(["\\Seen"]));
        let got = get(&c.pool, m.id).await.unwrap().unwrap();
        assert_eq!(got, m);
    }

    #[tokio::test]
    async fn test_unique_folder_uid() {
        let c = ctx().await;
        create(&c.pool, &sample(&c, 1)).await.unwrap();
        let err = create(&c.pool, &sample(&c, 1)).await.unwrap_err();
        assert!(err.to_string().to_lowercase().contains("unique"));
    }

    #[tokio::test]
    async fn test_get_by_folder_uid() {
        let c = ctx().await;
        let m = create(&c.pool, &sample(&c, 7)).await.unwrap();
        let got = get_by_folder_uid(&c.pool, c.folder_id, 7)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.id, m.id);
        assert!(get_by_folder_uid(&c.pool, c.folder_id, 8)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_list_by_folder_orders_newest_first() {
        let c = ctx().await;
        let mut a = sample(&c, 1);
        a.internal_date = Utc::now() - chrono::Duration::hours(2);
        let mut b = sample(&c, 2);
        b.internal_date = Utc::now() - chrono::Duration::hours(1);
        let mut d = sample(&c, 3);
        d.internal_date = Utc::now();
        create(&c.pool, &a).await.unwrap();
        create(&c.pool, &b).await.unwrap();
        create(&c.pool, &d).await.unwrap();
        let listed = list_by_folder(&c.pool, c.folder_id, 50, 0).await.unwrap();
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].uid, 3);
        assert_eq!(listed[2].uid, 1);
    }

    #[tokio::test]
    async fn test_list_by_folder_summary_excludes_bodies_and_maps_fields() {
        let c = ctx().await;
        let mut new = sample(&c, 42);
        new.html_body = Some("<p>secret html</p>".into());
        let created = create(&c.pool, &new).await.unwrap();

        let listed = list_by_folder(&c.pool, c.folder_id, 50, 0).await.unwrap();

        assert!(!LIST_BY_FOLDER_QUERY.contains("text_body"));
        assert!(!LIST_BY_FOLDER_QUERY.contains("html_body"));
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, created.id);
        assert_eq!(listed[0].account_id, c.account_id);
        assert_eq!(listed[0].folder_id, c.folder_id);
        assert_eq!(listed[0].thread_id, Some(c.thread_id));
        assert_eq!(listed[0].uid, 42);
        assert_eq!(listed[0].message_id_header.as_deref(), Some("<42@x>"));
        assert_eq!(listed[0].from_addr, "alice@x.com");
        assert_eq!(listed[0].to_addrs, json!(["bob@x.com"]));
        assert_eq!(listed[0].subject.as_deref(), Some("subject 42"));
        assert_eq!(listed[0].snippet.as_deref(), Some("hi"));
        assert_eq!(listed[0].raw_size, 1234);
        assert_eq!(listed[0].flags, json!(["\\Seen"]));
        assert_eq!(listed[0].internal_date, created.internal_date);
        assert_eq!(listed[0].created_at, created.created_at);
    }

    #[tokio::test]
    async fn test_list_by_thread_orders_oldest_first() {
        let c = ctx().await;
        let mut a = sample(&c, 1);
        a.internal_date = Utc::now() - chrono::Duration::hours(2);
        let mut b = sample(&c, 2);
        b.internal_date = Utc::now();
        create(&c.pool, &a).await.unwrap();
        create(&c.pool, &b).await.unwrap();
        let listed = list_by_thread(&c.pool, c.thread_id).await.unwrap();
        assert_eq!(listed[0].uid, 1);
        assert_eq!(listed[1].uid, 2);
        assert!(!LIST_BY_THREAD_QUERY.contains("text_body"));
        assert!(!LIST_BY_THREAD_QUERY.contains("html_body"));
    }

    #[tokio::test]
    async fn test_message_ids_by_threads_returns_non_null_ids_by_thread() {
        let c = ctx().await;
        let other = crate::db::threads::create(&c.pool, c.account_id, None, None)
            .await
            .unwrap();
        let empty = crate::db::threads::create(&c.pool, c.account_id, None, None)
            .await
            .unwrap();

        let mut older = sample(&c, 1);
        older.message_id_header = Some("<older@x>".into());
        older.internal_date = Utc::now() - chrono::Duration::hours(2);
        create(&c.pool, &older).await.unwrap();

        let mut without_header = sample(&c, 2);
        without_header.message_id_header = None;
        without_header.internal_date = Utc::now() - chrono::Duration::hours(1);
        create(&c.pool, &without_header).await.unwrap();

        let mut newer = sample(&c, 3);
        newer.message_id_header = Some("<newer@x>".into());
        newer.internal_date = Utc::now();
        create(&c.pool, &newer).await.unwrap();

        let mut other_message = sample(&c, 4);
        other_message.thread_id = Some(other.id);
        other_message.message_id_header = Some("<other@x>".into());
        create(&c.pool, &other_message).await.unwrap();

        let mut grouped = std::collections::HashMap::<ThreadId, Vec<String>>::new();
        for (thread_id, message_id) in
            message_ids_by_threads(&c.pool, &[c.thread_id, other.id, empty.id])
                .await
                .unwrap()
        {
            grouped.entry(thread_id).or_default().push(message_id);
        }

        assert_eq!(
            grouped.remove(&c.thread_id).unwrap(),
            vec!["<older@x>".to_string(), "<newer@x>".to_string()]
        );
        assert_eq!(
            grouped.remove(&other.id).unwrap(),
            vec!["<other@x>".to_string()]
        );
        assert!(!grouped.contains_key(&empty.id));
        assert!(grouped.is_empty());
        assert!(message_ids_by_threads(&c.pool, &[])
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn test_get_by_message_id_header() {
        let c = ctx().await;
        create(&c.pool, &sample(&c, 9)).await.unwrap();
        let got = get_by_message_id_header(&c.pool, c.account_id, "<9@x>")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.uid, 9);
        assert!(get_by_message_id_header(&c.pool, c.account_id, "<missing>")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_existing_uids_partitions() {
        let c = ctx().await;
        create(&c.pool, &sample(&c, 1)).await.unwrap();
        create(&c.pool, &sample(&c, 5)).await.unwrap();
        create(&c.pool, &sample(&c, 9)).await.unwrap();
        let got = existing_uids(&c.pool, c.folder_id, &[1, 2, 5, 7, 9])
            .await
            .unwrap();
        assert_eq!(got.len(), 3);
        for u in [1, 5, 9] {
            assert!(got.contains(&u));
        }
    }

    #[tokio::test]
    async fn test_existing_uids_empty_input() {
        let c = ctx().await;
        let got = existing_uids(&c.pool, c.folder_id, &[]).await.unwrap();
        assert!(got.is_empty());
    }

    #[tokio::test]
    async fn test_set_thread_and_flags() {
        let c = ctx().await;
        let m = create(&c.pool, &sample(&c, 1)).await.unwrap();
        let other = crate::db::threads::create(&c.pool, c.account_id, None, None)
            .await
            .unwrap();
        set_thread(&c.pool, m.id, Some(other.id)).await.unwrap();
        let got = get(&c.pool, m.id).await.unwrap().unwrap();
        assert_eq!(got.thread_id, Some(other.id));

        let outcome = set_flags(&c.pool, m.id, &json!(["\\Seen", "\\Flagged"]))
            .await
            .unwrap();
        assert_eq!(
            outcome,
            SetFlagsOutcome {
                found: true,
                changed: true,
            }
        );
        let got = get(&c.pool, m.id).await.unwrap().unwrap();
        assert_eq!(got.flags, json!(["\\Seen", "\\Flagged"]));
    }

    #[tokio::test]
    async fn test_set_flags_repeated_same_value_reports_unchanged() {
        let c = ctx().await;
        let m = create(&c.pool, &sample(&c, 1)).await.unwrap();
        let flags = json!(["\\Seen", "\\Flagged"]);

        let changed = set_flags(&c.pool, m.id, &flags).await.unwrap();
        assert_eq!(
            changed,
            SetFlagsOutcome {
                found: true,
                changed: true,
            }
        );
        let changes_after_first = total_changes(&c.pool).await;

        let unchanged = set_flags(&c.pool, m.id, &flags).await.unwrap();
        assert_eq!(
            unchanged,
            SetFlagsOutcome {
                found: true,
                changed: false,
            }
        );
        assert_eq!(total_changes(&c.pool).await, changes_after_first);

        let missing = set_flags(&c.pool, MessageId::new(), &flags).await.unwrap();
        assert_eq!(
            missing,
            SetFlagsOutcome {
                found: false,
                changed: false,
            }
        );
        assert_eq!(total_changes(&c.pool).await, changes_after_first);
    }

    async fn total_changes(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT total_changes()")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn test_delete_by_folder_uid() {
        let c = ctx().await;
        create(&c.pool, &sample(&c, 1)).await.unwrap();
        assert!(delete_by_folder_uid(&c.pool, c.folder_id, 1).await.unwrap());
        assert!(!delete_by_folder_uid(&c.pool, c.folder_id, 1).await.unwrap());
    }

    #[tokio::test]
    async fn test_set_thread_to_null_clears() {
        let c = ctx().await;
        let m = create(&c.pool, &sample(&c, 1)).await.unwrap();
        set_thread(&c.pool, m.id, None).await.unwrap();
        let got = get(&c.pool, m.id).await.unwrap().unwrap();
        assert!(got.thread_id.is_none());
    }

    #[tokio::test]
    async fn test_const_select_queries_run_against_empty_pool() {
        // Smoke-test every hoisted const query against an empty schema.
        // Catches column-list typos at runtime: SQLite errors out on an
        // unknown column even with zero rows.
        let c = ctx().await;
        assert!(get(&c.pool, MessageId::new()).await.unwrap().is_none());
        assert!(get_by_folder_uid(&c.pool, c.folder_id, 12345)
            .await
            .unwrap()
            .is_none());
        assert!(list_by_folder(&c.pool, c.folder_id, 50, 0)
            .await
            .unwrap()
            .is_empty());
        assert!(list_by_thread(&c.pool, c.thread_id)
            .await
            .unwrap()
            .is_empty());
        assert!(message_ids_by_threads(&c.pool, &[c.thread_id])
            .await
            .unwrap()
            .is_empty());
        assert!(get_by_message_id_header(&c.pool, c.account_id, "<missing>")
            .await
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_message_cols_const_lists_every_column() {
        // Column-list snapshot. Updating the column set is fine — bump
        // this snapshot in the same change.
        let cols: Vec<&str> = MESSAGE_COLS.split(',').map(|s| s.trim()).collect();
        assert_eq!(
            cols,
            vec![
                "id",
                "account_id",
                "folder_id",
                "thread_id",
                "uid",
                "message_id_header",
                "in_reply_to",
                "references_header",
                "from_addr",
                "to_addrs",
                "cc_addrs",
                "bcc_addrs",
                "reply_to",
                "subject",
                "snippet",
                "text_body",
                "html_body",
                "raw_size",
                "flags",
                "internal_date",
                "sent_at",
                "created_at",
            ]
        );
    }

    #[test]
    fn test_message_cols_const_lists_every_column_aliased() {
        let cols: Vec<&str> = MESSAGE_COLS_ALIASED_M
            .split(',')
            .map(|s| s.trim())
            .collect();
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

    #[test]
    fn test_message_summary_cols_const_excludes_body_columns() {
        let cols: Vec<&str> = MESSAGE_SUMMARY_COLS.split(',').map(|s| s.trim()).collect();
        assert_eq!(
            cols,
            vec![
                "id",
                "account_id",
                "folder_id",
                "thread_id",
                "uid",
                "message_id_header",
                "in_reply_to",
                "references_header",
                "from_addr",
                "to_addrs",
                "cc_addrs",
                "bcc_addrs",
                "reply_to",
                "subject",
                "snippet",
                "raw_size",
                "flags",
                "internal_date",
                "sent_at",
                "created_at",
            ]
        );
        assert!(!cols.contains(&"text_body"));
        assert!(!cols.contains(&"html_body"));
    }

    #[test]
    fn test_message_summary_cols_const_excludes_body_columns_aliased() {
        let cols: Vec<&str> = MESSAGE_SUMMARY_COLS_ALIASED_M
            .split(',')
            .map(|s| s.trim())
            .collect();
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
    async fn test_set_folder_moves_message_and_reports_match() {
        let c = ctx().await;
        let m = create(&c.pool, &sample(&c, 1)).await.unwrap();
        let other = crate::db::folders::create(
            &c.pool,
            &crate::db::folders::NewFolder {
                account_id: c.account_id,
                name: "Archive".into(),
                delimiter: "/".into(),
                role: crate::models::FolderRole::Archive,
                selectable: true,
            },
        )
        .await
        .unwrap();

        assert!(set_folder(&c.pool, m.id, other.id).await.unwrap());
        let got = get(&c.pool, m.id).await.unwrap().unwrap();
        assert_eq!(got.folder_id, other.id);
        assert!(!set_folder(&c.pool, MessageId::new(), other.id)
            .await
            .unwrap());
    }
}
