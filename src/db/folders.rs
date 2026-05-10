//! CRUD for the `folders` table.
//!
//! One row per IMAP mailbox node belonging to an account. Tracks the
//! server's hierarchy delimiter, the inferred [`FolderRole`], and the
//! UID-state triple (`uid_validity`, `uid_next`, `last_seen_uid`) the
//! sync layer uses to drive incremental fetches.

use serde::Deserialize;
use sqlx::SqlitePool;

use crate::db::DbError;
use crate::models::{AccountId, Folder, FolderId, FolderRole};

/// Input record for [`create`] / [`upsert`]: every column needed to
/// insert a new row into the `folders` table.
#[derive(Debug, Clone, Deserialize)]
pub struct NewFolder {
    /// Account this folder belongs to.
    pub account_id: AccountId,
    /// Server-reported folder name (e.g. `"INBOX"`).
    pub name: String,
    /// IMAP hierarchy delimiter reported by the server.
    pub delimiter: String,
    /// Inferred semantic role for the folder.
    pub role: FolderRole,
    /// Whether the folder can be `SELECT`ed (versus a container-only node).
    pub selectable: bool,
}

const SELECT: &str = "\
    id, account_id, name, delimiter, role, uid_validity, uid_next, last_seen_uid, \
    selectable, created_at";

/// Insert a folder row and return the persisted record.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the insert fails — typically a FK
/// violation when `account_id` is unknown or a `UNIQUE` violation on
/// `(account_id, name)`, but also any other SQLite error.
pub async fn create(pool: &SqlitePool, new: &NewFolder) -> Result<Folder, DbError> {
    let id = FolderId::new();
    let q = format!(
        "INSERT INTO folders (id, account_id, name, delimiter, role, selectable) \
         VALUES (?,?,?,?,?,?) RETURNING {SELECT}"
    );
    Ok(sqlx::query_as(&q)
        .bind(id)
        .bind(new.account_id)
        .bind(&new.name)
        .bind(&new.delimiter)
        .bind(new.role)
        .bind(new.selectable)
        .fetch_one(pool)
        .await?)
}

/// Insert if missing, otherwise update name/delimiter/role/selectable.
/// Used by IMAP LIST sync.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if any of the look-up, insert, update, or
/// post-update reload fails.
///
/// # Panics
///
/// Panics with a `BUG:` message if the row that was just upserted
/// cannot be re-read from the same connection — that would imply a
/// concurrent delete inside the same transaction window, which is not
/// possible with this code path.
pub async fn upsert(pool: &SqlitePool, new: &NewFolder) -> Result<Folder, DbError> {
    if let Some(existing) = get_by_name(pool, new.account_id, &new.name).await? {
        sqlx::query("UPDATE folders SET delimiter = ?, role = ?, selectable = ? WHERE id = ?")
            .bind(&new.delimiter)
            .bind(new.role)
            .bind(new.selectable)
            .bind(existing.id)
            .execute(pool)
            .await?;
        return Ok(get(pool, existing.id)
            .await?
            .expect("BUG: folder upserted moments ago must exist"));
    }
    create(pool, new).await
}

/// List folders for an account, alphabetically by name.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the query or row decode fails.
pub async fn list_by_account(
    pool: &SqlitePool,
    account_id: AccountId,
) -> Result<Vec<Folder>, DbError> {
    let q = format!("SELECT {SELECT} FROM folders WHERE account_id = ? ORDER BY name");
    Ok(sqlx::query_as(&q).bind(account_id).fetch_all(pool).await?)
}

/// Look up a folder by id; `Ok(None)` if missing.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the query or row decode fails. A missing
/// row is reported as `Ok(None)`, not an error.
pub async fn get(pool: &SqlitePool, id: FolderId) -> Result<Option<Folder>, DbError> {
    let q = format!("SELECT {SELECT} FROM folders WHERE id = ?");
    Ok(sqlx::query_as(&q).bind(id).fetch_optional(pool).await?)
}

/// Look up a folder by `(account_id, name)`; `Ok(None)` if missing.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the query or row decode fails. A missing
/// row is reported as `Ok(None)`, not an error.
pub async fn get_by_name(
    pool: &SqlitePool,
    account_id: AccountId,
    name: &str,
) -> Result<Option<Folder>, DbError> {
    let q = format!("SELECT {SELECT} FROM folders WHERE account_id = ? AND name = ?");
    Ok(sqlx::query_as(&q)
        .bind(account_id)
        .bind(name)
        .fetch_optional(pool)
        .await?)
}

/// Update the IMAP UID state for a folder. Each `Option` argument writes
/// only when `Some`; `None` preserves the existing value.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the update fails. A missing id is a
/// silent no-op (rows_affected = 0), not an error.
pub async fn update_uid_state(
    pool: &SqlitePool,
    id: FolderId,
    uid_validity: Option<i64>,
    uid_next: Option<i64>,
    last_seen_uid: Option<i64>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE folders SET uid_validity = COALESCE(?, uid_validity), \
         uid_next = COALESCE(?, uid_next), \
         last_seen_uid = COALESCE(?, last_seen_uid) WHERE id = ?",
    )
    .bind(uid_validity)
    .bind(uid_next)
    .bind(last_seen_uid)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Delete a folder by id. Returns `true` if a row was removed.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the delete fails (FK or IO). A missing
/// row is reported as `Ok(false)`, not an error.
pub async fn delete(pool: &SqlitePool, id: FolderId) -> Result<bool, DbError> {
    let r = sqlx::query("DELETE FROM folders WHERE id = ?")
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
        let acc = crate::db::accounts::create(
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
        .unwrap();
        acc.id
    }

    fn folder(account_id: AccountId, name: &str, role: FolderRole) -> NewFolder {
        NewFolder {
            account_id,
            name: name.into(),
            delimiter: "/".into(),
            role,
            selectable: true,
        }
    }

    #[tokio::test]
    async fn test_create_and_get() {
        let pool = crate::db::test_pool().await;
        let acc = account(&pool).await;
        let f = create(&pool, &folder(acc, "INBOX", FolderRole::Inbox))
            .await
            .unwrap();
        assert_eq!(f.role, FolderRole::Inbox);
        assert!(f.selectable);
        let got = get(&pool, f.id).await.unwrap().unwrap();
        assert_eq!(got, f);
    }

    #[tokio::test]
    async fn test_upsert_inserts_then_updates() {
        let pool = crate::db::test_pool().await;
        let acc = account(&pool).await;
        let f1 = upsert(&pool, &folder(acc, "INBOX", FolderRole::Inbox))
            .await
            .unwrap();

        let mut nf = folder(acc, "INBOX", FolderRole::Custom);
        nf.delimiter = ".".into();
        nf.selectable = false;
        let f2 = upsert(&pool, &nf).await.unwrap();

        assert_eq!(f1.id, f2.id);
        assert_eq!(f2.role, FolderRole::Custom);
        assert_eq!(f2.delimiter, ".");
        assert!(!f2.selectable);
    }

    #[tokio::test]
    async fn test_unique_per_account() {
        let pool = crate::db::test_pool().await;
        let acc = account(&pool).await;
        create(&pool, &folder(acc, "INBOX", FolderRole::Inbox))
            .await
            .unwrap();
        let err = create(&pool, &folder(acc, "INBOX", FolderRole::Inbox))
            .await
            .unwrap_err();
        assert!(err.to_string().to_lowercase().contains("unique"));
    }

    #[tokio::test]
    async fn test_list_by_account_filters() {
        let pool = crate::db::test_pool().await;
        let a = account(&pool).await;
        let b = account(&pool).await;
        create(&pool, &folder(a, "INBOX", FolderRole::Inbox))
            .await
            .unwrap();
        create(&pool, &folder(a, "Sent", FolderRole::Sent))
            .await
            .unwrap();
        create(&pool, &folder(b, "INBOX", FolderRole::Inbox))
            .await
            .unwrap();
        let listed = list_by_account(&pool, a).await.unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().all(|f| f.account_id == a));
    }

    #[tokio::test]
    async fn test_update_uid_state_only_writes_provided() {
        let pool = crate::db::test_pool().await;
        let acc = account(&pool).await;
        let f = create(&pool, &folder(acc, "INBOX", FolderRole::Inbox))
            .await
            .unwrap();
        update_uid_state(&pool, f.id, Some(42), Some(100), Some(99))
            .await
            .unwrap();
        let got = get(&pool, f.id).await.unwrap().unwrap();
        assert_eq!(got.uid_validity, Some(42));
        assert_eq!(got.uid_next, Some(100));
        assert_eq!(got.last_seen_uid, Some(99));

        // Only update last_seen_uid; others preserved.
        update_uid_state(&pool, f.id, None, None, Some(120))
            .await
            .unwrap();
        let got = get(&pool, f.id).await.unwrap().unwrap();
        assert_eq!(got.uid_validity, Some(42));
        assert_eq!(got.uid_next, Some(100));
        assert_eq!(got.last_seen_uid, Some(120));
    }

    #[tokio::test]
    async fn test_delete_cascade_via_account() {
        let pool = crate::db::test_pool().await;
        let acc = account(&pool).await;
        create(&pool, &folder(acc, "INBOX", FolderRole::Inbox))
            .await
            .unwrap();
        crate::db::accounts::delete(&pool, acc).await.unwrap();
        let listed = list_by_account(&pool, acc).await.unwrap();
        assert!(listed.is_empty());
    }
}
