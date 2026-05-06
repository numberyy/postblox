use serde::Deserialize;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::models::{Folder, FolderRole};

#[derive(Debug, Clone, Deserialize)]
pub struct NewFolder {
    pub account_id: Uuid,
    pub name: String,
    pub delimiter: String,
    pub role: FolderRole,
    pub selectable: bool,
}

const SELECT: &str = "\
    id, account_id, name, delimiter, role, uid_validity, uid_next, last_seen_uid, \
    selectable, created_at";

pub async fn create(pool: &SqlitePool, new: &NewFolder) -> Result<Folder, sqlx::Error> {
    let id = Uuid::new_v4();
    let q = format!(
        "INSERT INTO folders (id, account_id, name, delimiter, role, selectable) \
         VALUES (?,?,?,?,?,?) RETURNING {SELECT}"
    );
    sqlx::query_as(&q)
        .bind(id)
        .bind(new.account_id)
        .bind(&new.name)
        .bind(&new.delimiter)
        .bind(new.role)
        .bind(new.selectable)
        .fetch_one(pool)
        .await
}

/// Insert if missing, otherwise update name/delimiter/role/selectable.
/// Used by IMAP LIST sync.
pub async fn upsert(pool: &SqlitePool, new: &NewFolder) -> Result<Folder, sqlx::Error> {
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
            .expect("folder we just updated"));
    }
    create(pool, new).await
}

pub async fn list_by_account(
    pool: &SqlitePool,
    account_id: Uuid,
) -> Result<Vec<Folder>, sqlx::Error> {
    let q = format!("SELECT {SELECT} FROM folders WHERE account_id = ? ORDER BY name");
    sqlx::query_as(&q).bind(account_id).fetch_all(pool).await
}

pub async fn get(pool: &SqlitePool, id: Uuid) -> Result<Option<Folder>, sqlx::Error> {
    let q = format!("SELECT {SELECT} FROM folders WHERE id = ?");
    sqlx::query_as(&q).bind(id).fetch_optional(pool).await
}

pub async fn get_by_name(
    pool: &SqlitePool,
    account_id: Uuid,
    name: &str,
) -> Result<Option<Folder>, sqlx::Error> {
    let q = format!("SELECT {SELECT} FROM folders WHERE account_id = ? AND name = ?");
    sqlx::query_as(&q)
        .bind(account_id)
        .bind(name)
        .fetch_optional(pool)
        .await
}

pub async fn update_uid_state(
    pool: &SqlitePool,
    id: Uuid,
    uid_validity: Option<i64>,
    uid_next: Option<i64>,
    last_seen_uid: Option<i64>,
) -> Result<(), sqlx::Error> {
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

pub async fn delete(pool: &SqlitePool, id: Uuid) -> Result<bool, sqlx::Error> {
    let r = sqlx::query("DELETE FROM folders WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn account(pool: &SqlitePool) -> Uuid {
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

    fn folder(account_id: Uuid, name: &str, role: FolderRole) -> NewFolder {
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
