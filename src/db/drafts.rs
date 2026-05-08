use serde::Deserialize;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::models::Draft;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NewDraft {
    pub account_id: Uuid,
    pub in_reply_to_msg: Option<Uuid>,
    pub to_addrs: serde_json::Value,
    pub cc_addrs: serde_json::Value,
    pub bcc_addrs: serde_json::Value,
    pub subject: Option<String>,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
    #[serde(default)]
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub references_header: Option<String>,
}

const SELECT: &str = "\
    id, account_id, in_reply_to_msg, to_addrs, cc_addrs, bcc_addrs, subject, \
    text_body, html_body, in_reply_to, references_header, remote_folder_id, \
    remote_uid, created_at, updated_at";

pub async fn create(pool: &SqlitePool, new: &NewDraft) -> Result<Draft, sqlx::Error> {
    let id = Uuid::new_v4();
    let q = format!(
        "INSERT INTO drafts \
         (id, account_id, in_reply_to_msg, to_addrs, cc_addrs, bcc_addrs, \
          subject, text_body, html_body, in_reply_to, references_header) \
         VALUES (?,?,?,?,?,?,?,?,?,?,?) RETURNING {SELECT}"
    );
    sqlx::query_as(&q)
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
        .await
}

#[derive(Debug, Clone)]
pub struct DraftPatch<'a> {
    pub to_addrs: &'a serde_json::Value,
    pub cc_addrs: &'a serde_json::Value,
    pub bcc_addrs: &'a serde_json::Value,
    pub subject: Option<&'a str>,
    pub text_body: Option<&'a str>,
    pub html_body: Option<&'a str>,
}

pub async fn update(
    pool: &SqlitePool,
    id: Uuid,
    patch: &DraftPatch<'_>,
) -> Result<Option<Draft>, sqlx::Error> {
    let q = format!(
        "UPDATE drafts SET to_addrs=?, cc_addrs=?, bcc_addrs=?, subject=?, \
         text_body=?, html_body=?, \
         updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') \
         WHERE id=? RETURNING {SELECT}"
    );
    sqlx::query_as(&q)
        .bind(patch.to_addrs)
        .bind(patch.cc_addrs)
        .bind(patch.bcc_addrs)
        .bind(patch.subject)
        .bind(patch.text_body)
        .bind(patch.html_body)
        .bind(id)
        .fetch_optional(pool)
        .await
}

pub async fn set_remote(
    pool: &SqlitePool,
    id: Uuid,
    folder_id: Uuid,
    uid: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE drafts SET remote_folder_id = ?, remote_uid = ? WHERE id = ?")
        .bind(folder_id)
        .bind(uid)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn get(pool: &SqlitePool, id: Uuid) -> Result<Option<Draft>, sqlx::Error> {
    let q = format!("SELECT {SELECT} FROM drafts WHERE id = ?");
    sqlx::query_as(&q).bind(id).fetch_optional(pool).await
}

pub async fn list_by_account(
    pool: &SqlitePool,
    account_id: Uuid,
) -> Result<Vec<Draft>, sqlx::Error> {
    let q = format!("SELECT {SELECT} FROM drafts WHERE account_id = ? ORDER BY updated_at DESC");
    sqlx::query_as(&q).bind(account_id).fetch_all(pool).await
}

pub async fn delete(pool: &SqlitePool, id: Uuid) -> Result<bool, sqlx::Error> {
    let r = sqlx::query("DELETE FROM drafts WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn account(pool: &SqlitePool) -> Uuid {
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

    fn sample(account_id: Uuid) -> NewDraft {
        NewDraft {
            account_id,
            in_reply_to_msg: None,
            to_addrs: json!(["bob@x.com"]),
            cc_addrs: json!([]),
            bcc_addrs: json!([]),
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
        let to = json!(["c@x.com"]);
        let cc = json!([]);
        let bcc = json!([]);
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
        assert_eq!(updated.to_addrs, json!(["c@x.com"]));
        assert!(updated.updated_at >= d.updated_at);
    }

    #[tokio::test]
    async fn test_update_unknown_returns_none() {
        let pool = crate::db::test_pool().await;
        let empty = json!([]);
        let res = update(
            &pool,
            Uuid::new_v4(),
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
