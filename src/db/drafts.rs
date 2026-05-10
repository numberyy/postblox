//! CRUD for the `drafts` table.
//!
//! Backs the TUI composer and the MCP `draft.*` tools: in-progress
//! messages persist between restarts and can be remote-anchored
//! (`remote_folder_id` / `remote_uid`) once IMAP `APPEND` succeeds.
//! `in_reply_to_msg` links replies to the source message; the raw
//! `In-Reply-To` and `References` headers ride alongside so the
//! eventual MIME builder can emit RFC 5322 §3.6.4 threading without
//! re-deriving them.

use serde::Deserialize;
use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::db::DbError;
use crate::models::{AccountId, AddressList, Draft, DraftId, FolderId, MessageId};

/// Input record for [`create`]: every column needed to insert a new
/// row into the `drafts` table.
#[derive(Debug, Clone, Deserialize)]
pub struct NewDraft {
    /// Account the draft will be sent from.
    pub account_id: AccountId,
    /// Local message this draft replies to, if any.
    pub in_reply_to_msg: Option<MessageId>,
    /// `To` recipients.
    pub to_addrs: AddressList,
    /// `Cc` recipients.
    pub cc_addrs: AddressList,
    /// `Bcc` recipients.
    pub bcc_addrs: AddressList,
    /// `Subject` header value.
    pub subject: Option<String>,
    /// Plain-text body.
    pub text_body: Option<String>,
    /// HTML body.
    pub html_body: Option<String>,
    /// `In-Reply-To` header for outbound threading.
    #[serde(default)]
    pub in_reply_to: Option<String>,
    /// `References` header for outbound threading.
    #[serde(default)]
    pub references_header: Option<String>,
}

const SELECT: &str = "\
    id, account_id, in_reply_to_msg, to_addrs, cc_addrs, bcc_addrs, subject, \
    text_body, html_body, in_reply_to, references_header, remote_folder_id, \
    remote_uid, created_at, updated_at";

/// Insert a new draft row and return the persisted record.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the insert fails — typically a FK
/// violation when `account_id` (or `in_reply_to_msg`) is unknown, or
/// any other SQLite error.
pub async fn create(pool: &SqlitePool, new: &NewDraft) -> Result<Draft, DbError> {
    let id = DraftId::new();
    let q = format!(
        "INSERT INTO drafts \
         (id, account_id, in_reply_to_msg, to_addrs, cc_addrs, bcc_addrs, \
          subject, text_body, html_body, in_reply_to, references_header) \
         VALUES (?,?,?,?,?,?,?,?,?,?,?) RETURNING {SELECT}"
    );
    Ok(sqlx::query_as(&q)
        .bind(id)
        .bind(new.account_id)
        .bind(new.in_reply_to_msg)
        .bind(&new.to_addrs)
        .bind(&new.cc_addrs)
        .bind(&new.bcc_addrs)
        .bind(&new.subject)
        .bind(&new.text_body)
        .bind(&new.html_body)
        .bind(&new.in_reply_to)
        .bind(&new.references_header)
        .fetch_one(pool)
        .await?)
}

/// Borrowed patch applied by [`update`] and [`update_tx`].
#[derive(Debug, Clone)]
pub struct DraftPatch<'a> {
    /// New `To` recipients.
    pub to_addrs: &'a AddressList,
    /// New `Cc` recipients.
    pub cc_addrs: &'a AddressList,
    /// New `Bcc` recipients.
    pub bcc_addrs: &'a AddressList,
    /// New `Subject`, or `None` to clear.
    pub subject: Option<&'a str>,
    /// New plain-text body, or `None` to clear.
    pub text_body: Option<&'a str>,
    /// New HTML body, or `None` to clear.
    pub html_body: Option<&'a str>,
}

/// Apply a patch to a draft and return the updated row, or `Ok(None)`
/// if no row matched the id.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the update fails. A missing id is
/// reported as `Ok(None)`, not an error.
pub async fn update(
    pool: &SqlitePool,
    id: DraftId,
    patch: &DraftPatch<'_>,
) -> Result<Option<Draft>, DbError> {
    let q = format!(
        "UPDATE drafts SET to_addrs=?, cc_addrs=?, bcc_addrs=?, subject=?, \
         text_body=?, html_body=?, \
         updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') \
         WHERE id=? RETURNING {SELECT}"
    );
    Ok(sqlx::query_as(&q)
        .bind(patch.to_addrs)
        .bind(patch.cc_addrs)
        .bind(patch.bcc_addrs)
        .bind(patch.subject)
        .bind(patch.text_body)
        .bind(patch.html_body)
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// Transactional variant of [`update`].
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the update fails. A missing id is
/// reported as `Ok(None)`, not an error.
pub async fn update_tx(
    tx: &mut Transaction<'_, Sqlite>,
    id: DraftId,
    patch: &DraftPatch<'_>,
) -> Result<Option<Draft>, DbError> {
    let q = format!(
        "UPDATE drafts SET to_addrs=?, cc_addrs=?, bcc_addrs=?, subject=?, \
         text_body=?, html_body=?, \
         updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') \
         WHERE id=? RETURNING {SELECT}"
    );
    Ok(sqlx::query_as(&q)
        .bind(patch.to_addrs)
        .bind(patch.cc_addrs)
        .bind(patch.bcc_addrs)
        .bind(patch.subject)
        .bind(patch.text_body)
        .bind(patch.html_body)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?)
}

/// Mark a draft as synced to a remote folder/uid pair.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the update fails (FK violation when
/// `folder_id` is unknown, or any other SQLite error). A missing
/// `id` is a silent no-op (rows_affected = 0).
pub async fn set_remote(
    pool: &SqlitePool,
    id: DraftId,
    folder_id: FolderId,
    uid: i64,
) -> Result<(), DbError> {
    sqlx::query("UPDATE drafts SET remote_folder_id = ?, remote_uid = ? WHERE id = ?")
        .bind(folder_id)
        .bind(uid)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Look up a draft by id; `Ok(None)` if missing.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the query or row decode fails. A missing
/// row is reported as `Ok(None)`, not an error.
pub async fn get(pool: &SqlitePool, id: DraftId) -> Result<Option<Draft>, DbError> {
    let q = format!("SELECT {SELECT} FROM drafts WHERE id = ?");
    Ok(sqlx::query_as(&q).bind(id).fetch_optional(pool).await?)
}

/// List drafts for an account, most-recently updated first.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the query or row decode fails.
pub async fn list_by_account(
    pool: &SqlitePool,
    account_id: AccountId,
) -> Result<Vec<Draft>, DbError> {
    let q = format!("SELECT {SELECT} FROM drafts WHERE account_id = ? ORDER BY updated_at DESC");
    Ok(sqlx::query_as(&q).bind(account_id).fetch_all(pool).await?)
}

/// Delete a draft by id. Returns `true` if a row was removed.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the delete fails. A missing row is
/// reported as `Ok(false)`, not an error.
pub async fn delete(pool: &SqlitePool, id: DraftId) -> Result<bool, DbError> {
    let r = sqlx::query("DELETE FROM drafts WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    async fn account(pool: &SqlitePool) -> AccountId {
        crate::db::accounts::create(
            pool,
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
        .unwrap()
        .id
    }

    fn sample(account_id: AccountId) -> NewDraft {
        NewDraft {
            account_id,
            in_reply_to_msg: None,
            to_addrs: AddressList::from(vec!["bob@x.com"]),
            cc_addrs: AddressList::default(),
            bcc_addrs: AddressList::default(),
            subject: Some("Hi".into()),
            text_body: Some("Hello".into()),
            html_body: None,
            in_reply_to: None,
            references_header: None,
        }
    }

    #[tokio::test]
    async fn test_create_get() {
        let pool = crate::db::test_pool().await;
        let a = account(&pool).await;
        let d = create(&pool, &sample(a)).await.unwrap();
        assert_eq!(d.subject.as_deref(), Some("Hi"));
        assert!(d.remote_uid.is_none());
        let got = get(&pool, d.id).await.unwrap().unwrap();
        assert_eq!(got, d);
    }

    #[tokio::test]
    async fn test_update_changes_fields_and_updated_at() {
        let pool = crate::db::test_pool().await;
        let a = account(&pool).await;
        let d = create(&pool, &sample(a)).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let to = AddressList::from(vec!["c@x.com"]);
        let cc = AddressList::default();
        let bcc = AddressList::default();
        let updated = update(
            &pool,
            d.id,
            &DraftPatch {
                to_addrs: &to,
                cc_addrs: &cc,
                bcc_addrs: &bcc,
                subject: Some("New subject"),
                text_body: Some("Body 2"),
                html_body: None,
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(updated.subject.as_deref(), Some("New subject"));
        assert_eq!(updated.to_addrs, AddressList::from(vec!["c@x.com"]));
        assert!(updated.updated_at >= d.updated_at);
    }

    #[tokio::test]
    async fn test_update_unknown_returns_none() {
        let pool = crate::db::test_pool().await;
        let empty = AddressList::default();
        let res = update(
            &pool,
            DraftId::new(),
            &DraftPatch {
                to_addrs: &empty,
                cc_addrs: &empty,
                bcc_addrs: &empty,
                subject: None,
                text_body: None,
                html_body: None,
            },
        )
        .await
        .unwrap();
        assert!(res.is_none());
    }

    #[tokio::test]
    async fn test_set_remote_marks_synced() {
        let pool = crate::db::test_pool().await;
        let a = account(&pool).await;
        let d = create(&pool, &sample(a)).await.unwrap();
        let folder = crate::db::folders::create(
            &pool,
            &crate::db::folders::NewFolder {
                account_id: a,
                name: "Drafts".into(),
                delimiter: "/".into(),
                role: crate::models::FolderRole::Drafts,
                selectable: true,
            },
        )
        .await
        .unwrap();
        set_remote(&pool, d.id, folder.id, 17).await.unwrap();
        let got = get(&pool, d.id).await.unwrap().unwrap();
        assert_eq!(got.remote_folder_id, Some(folder.id));
        assert_eq!(got.remote_uid, Some(17));
    }

    #[tokio::test]
    async fn test_list_orders_by_updated_desc() {
        let pool = crate::db::test_pool().await;
        let a = account(&pool).await;
        let first = create(&pool, &sample(a)).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let second = create(&pool, &sample(a)).await.unwrap();
        let listed = list_by_account(&pool, a).await.unwrap();
        assert_eq!(listed[0].id, second.id);
        assert_eq!(listed[1].id, first.id);
    }
}
