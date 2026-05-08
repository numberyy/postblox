use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::models::Message;

#[derive(Debug, Clone)]
pub struct NewMessage {
    pub account_id: Uuid,
    pub folder_id: Uuid,
    pub thread_id: Option<Uuid>,
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

const SELECT: &str = "\
    id, account_id, folder_id, thread_id, uid, message_id_header, in_reply_to, \
    references_header, from_addr, to_addrs, cc_addrs, bcc_addrs, reply_to, \
    subject, snippet, text_body, html_body, raw_size, flags, internal_date, \
    sent_at, created_at";

pub async fn create(pool: &SqlitePool, new: &NewMessage) -> Result<Message, sqlx::Error> {
    let id = Uuid::new_v4();
    let q = format!(
        "INSERT INTO messages \
         (id, account_id, folder_id, thread_id, uid, message_id_header, in_reply_to, \
          references_header, from_addr, to_addrs, cc_addrs, bcc_addrs, reply_to, \
          subject, snippet, text_body, html_body, raw_size, flags, internal_date, sent_at) \
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) RETURNING {SELECT}"
    );
    sqlx::query_as(&q)
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
        .await
}

pub async fn get(pool: &SqlitePool, id: Uuid) -> Result<Option<Message>, sqlx::Error> {
    let q = format!("SELECT {SELECT} FROM messages WHERE id = ?");
    sqlx::query_as(&q).bind(id).fetch_optional(pool).await
}

pub async fn get_by_folder_uid(
    pool: &SqlitePool,
    folder_id: Uuid,
    uid: i64,
) -> Result<Option<Message>, sqlx::Error> {
    let q = format!("SELECT {SELECT} FROM messages WHERE folder_id = ? AND uid = ?");
    sqlx::query_as(&q)
        .bind(folder_id)
        .bind(uid)
        .fetch_optional(pool)
        .await
}

pub async fn list_by_folder(
    pool: &SqlitePool,
    folder_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<Message>, sqlx::Error> {
    let q = format!(
        "SELECT {SELECT} FROM messages WHERE folder_id = ? \
         ORDER BY internal_date DESC LIMIT ? OFFSET ?"
    );
    sqlx::query_as(&q)
        .bind(folder_id)
        .bind(limit.clamp(1, 500))
        .bind(offset.max(0))
        .fetch_all(pool)
        .await
}

pub async fn list_by_thread(
    pool: &SqlitePool,
    thread_id: Uuid,
) -> Result<Vec<Message>, sqlx::Error> {
    let q = format!("SELECT {SELECT} FROM messages WHERE thread_id = ? ORDER BY internal_date");
    sqlx::query_as(&q).bind(thread_id).fetch_all(pool).await
}

/// Find a message by its RFC822 Message-ID header within an account. Used
/// by the threading matcher to walk In-Reply-To / References chains.
pub async fn find_by_msgid_header(
    pool: &SqlitePool,
    account_id: Uuid,
    message_id_header: &str,
) -> Result<Option<Message>, sqlx::Error> {
    let q = format!(
        "SELECT {SELECT} FROM messages \
         WHERE account_id = ? AND message_id_header = ? LIMIT 1"
    );
    sqlx::query_as(&q)
        .bind(account_id)
        .bind(message_id_header)
        .fetch_optional(pool)
        .await
}

/// Return the subset of `uids` that already exist in `folder_id`. Used by
/// IMAP sync to skip messages we've already fetched.
pub async fn existing_uids(
    pool: &SqlitePool,
    folder_id: Uuid,
    uids: &[i64],
) -> Result<std::collections::HashSet<i64>, sqlx::Error> {
    if uids.is_empty() {
        return Ok(Default::default());
    }
    let placeholders = vec!["?"; uids.len()].join(",");
    let q = format!("SELECT uid FROM messages WHERE folder_id = ? AND uid IN ({placeholders})");
    let mut query = sqlx::query_as::<_, (i64,)>(&q).bind(folder_id);
    for u in uids {
        query = query.bind(u);
    }
    let rows = query.fetch_all(pool).await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

pub async fn set_thread(
    pool: &SqlitePool,
    id: Uuid,
    thread_id: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE messages SET thread_id = ? WHERE id = ?")
        .bind(thread_id)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_flags(
    pool: &SqlitePool,
    id: Uuid,
    flags: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE messages SET flags = ? WHERE id = ?")
        .bind(flags)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Reassign a message to a different folder. Returns true when a row
/// matched. Used by archive / move ops; on the wire IMAP this is what a
/// MOVE would do server-side, but here we reflect the change locally
/// and let the next IMAP sync reconcile.
pub async fn set_folder(pool: &SqlitePool, id: Uuid, folder_id: Uuid) -> Result<bool, sqlx::Error> {
    let r = sqlx::query("UPDATE messages SET folder_id = ? WHERE id = ?")
        .bind(folder_id)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}

pub async fn delete(pool: &SqlitePool, id: Uuid) -> Result<bool, sqlx::Error> {
    let r = sqlx::query("DELETE FROM messages WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}

pub async fn delete_by_folder_uid(
    pool: &SqlitePool,
    folder_id: Uuid,
    uid: i64,
) -> Result<bool, sqlx::Error> {
    let r = sqlx::query("DELETE FROM messages WHERE folder_id = ? AND uid = ?")
        .bind(folder_id)
        .bind(uid)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}

/// Wipe every message in a folder. Used when the server's
/// `UIDVALIDITY` changed under us and we have to refetch from scratch.
pub async fn delete_all_in_folder(pool: &SqlitePool, folder_id: Uuid) -> Result<u64, sqlx::Error> {
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

    struct Ctx {
        pool: SqlitePool,
        account_id: Uuid,
        folder_id: Uuid,
        thread_id: Uuid,
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
    }

    #[tokio::test]
    async fn test_find_by_msgid_header() {
        let c = ctx().await;
        create(&c.pool, &sample(&c, 9)).await.unwrap();
        let got = find_by_msgid_header(&c.pool, c.account_id, "<9@x>")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.uid, 9);
        assert!(find_by_msgid_header(&c.pool, c.account_id, "<missing>")
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

        set_flags(&c.pool, m.id, &json!(["\\Seen", "\\Flagged"]))
            .await
            .unwrap();
        let got = get(&c.pool, m.id).await.unwrap().unwrap();
        assert_eq!(got.flags, json!(["\\Seen", "\\Flagged"]));
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
        assert!(!set_folder(&c.pool, Uuid::new_v4(), other.id).await.unwrap());
    }
}
