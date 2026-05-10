//! Outgoing draft attachments — bytes stored inline in SQLite. Cap is
//! enforced by callers (see daemon dispatcher) so we never write past
//! the 25 MB hard limit set in `CLAUDE.md`.

use sqlx::{Sqlite, SqlitePool, Transaction};
use uuid::Uuid;

use crate::db::DbError;
use crate::models::{DraftAttachment, DraftId};

/// Maximum bytes for any single draft attachment (and the aggregate cap
/// per draft). Mirrors the `CLAUDE.md` "Attachment size: max 25 MB"
/// hard limit.
pub const MAX_DRAFT_ATTACHMENT_BYTES: i64 = 25 * 1024 * 1024;

const COLS: &str = "id, draft_id, filename, content_type, size_bytes, created_at";

#[derive(Debug, Clone)]
pub struct NewDraftAttachment {
    pub draft_id: DraftId,
    pub filename: String,
    pub content_type: String,
    pub content: Vec<u8>,
}

/// Insert a draft attachment row (bytes stored inline) and return the
/// metadata record.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the insert or the follow-up `SELECT` fails
/// (FK violation when `draft_id` is unknown, or any other SQLite error).
pub async fn create(
    pool: &SqlitePool,
    new: &NewDraftAttachment,
) -> Result<DraftAttachment, DbError> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO draft_attachments \
         (id, draft_id, filename, content_type, size_bytes, content) \
         VALUES (?,?,?,?,?,?)",
    )
    .bind(id)
    .bind(new.draft_id)
    .bind(&new.filename)
    .bind(&new.content_type)
    .bind(new.content.len() as i64)
    .bind(&new.content)
    .execute(pool)
    .await?;
    Ok(sqlx::query_as::<_, DraftAttachment>(&format!(
        "SELECT {COLS} FROM draft_attachments WHERE id = ?"
    ))
    .bind(id)
    .fetch_one(pool)
    .await?)
}

/// Transactional variant of [`create`].
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the insert or the follow-up `SELECT` fails
/// (FK violation when `draft_id` is unknown, or any other SQLite error).
pub async fn create_tx(
    tx: &mut Transaction<'_, Sqlite>,
    new: &NewDraftAttachment,
) -> Result<DraftAttachment, DbError> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO draft_attachments \
         (id, draft_id, filename, content_type, size_bytes, content) \
         VALUES (?,?,?,?,?,?)",
    )
    .bind(id)
    .bind(new.draft_id)
    .bind(&new.filename)
    .bind(&new.content_type)
    .bind(new.content.len() as i64)
    .bind(&new.content)
    .execute(&mut **tx)
    .await?;
    Ok(sqlx::query_as::<_, DraftAttachment>(&format!(
        "SELECT {COLS} FROM draft_attachments WHERE id = ?"
    ))
    .bind(id)
    .fetch_one(&mut **tx)
    .await?)
}

/// List the metadata for every attachment on a draft, oldest first.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the query or row decode fails. Unknown
/// `draft_id` returns `Ok(Vec::new())`, not an error.
pub async fn list_for_draft(
    pool: &SqlitePool,
    draft_id: DraftId,
) -> Result<Vec<DraftAttachment>, DbError> {
    Ok(sqlx::query_as::<_, DraftAttachment>(&format!(
        "SELECT {COLS} FROM draft_attachments WHERE draft_id = ? ORDER BY created_at, id"
    ))
    .bind(draft_id)
    .fetch_all(pool)
    .await?)
}

/// Load the inline attachment bytes for the given id; `Ok(None)` if missing.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the query or row decode fails. A missing
/// row is reported as `Ok(None)`, not an error.
pub async fn load_content(pool: &SqlitePool, id: Uuid) -> Result<Option<Vec<u8>>, DbError> {
    let row: Option<(Vec<u8>,)> =
        sqlx::query_as("SELECT content FROM draft_attachments WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|r| r.0))
}

/// Sum `size_bytes` across every attachment on a draft. Returns `0` when
/// there are no rows. Used by callers to enforce the per-draft cap.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the aggregate query fails.
pub async fn aggregate_size_for_draft(
    pool: &SqlitePool,
    draft_id: DraftId,
) -> Result<i64, DbError> {
    let row: (Option<i64>,) =
        sqlx::query_as("SELECT SUM(size_bytes) FROM draft_attachments WHERE draft_id = ?")
            .bind(draft_id)
            .fetch_one(pool)
            .await?;
    Ok(row.0.unwrap_or(0))
}

/// Delete every attachment for a draft. Returns the number of rows removed.
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the delete fails (FK or IO). Unknown
/// `draft_id` returns `Ok(0)`, not an error.
pub async fn delete_all_for_draft(pool: &SqlitePool, draft_id: DraftId) -> Result<u64, DbError> {
    let r = sqlx::query("DELETE FROM draft_attachments WHERE draft_id = ?")
        .bind(draft_id)
        .execute(pool)
        .await?;
    Ok(r.rows_affected())
}

/// Transactional variant of [`delete_all_for_draft`].
///
/// # Errors
///
/// Returns [`DbError::Sqlx`] if the delete fails (FK or IO). Unknown
/// `draft_id` returns `Ok(0)`, not an error.
pub async fn delete_all_for_draft_tx(
    tx: &mut Transaction<'_, Sqlite>,
    draft_id: DraftId,
) -> Result<u64, DbError> {
    let r = sqlx::query("DELETE FROM draft_attachments WHERE draft_id = ?")
        .bind(draft_id)
        .execute(&mut **tx)
        .await?;
    Ok(r.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{drafts, test_pool};
    use serde_json::json;

    async fn account_and_draft(pool: &SqlitePool) -> DraftId {
        let account = crate::db::accounts::create(
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
        let draft = drafts::create(
            pool,
            &drafts::NewDraft {
                account_id: account.id,
                in_reply_to_msg: None,
                to_addrs: json!(["bob@x.com"]),
                cc_addrs: json!([]),
                bcc_addrs: json!([]),
                subject: Some("hi".into()),
                text_body: Some("body".into()),
                html_body: None,
                in_reply_to: None,
                references_header: None,
            },
        )
        .await
        .unwrap();
        draft.id
    }

    #[tokio::test]
    async fn test_create_lists_and_loads_content() {
        let pool = test_pool().await;
        let draft_id = account_and_draft(&pool).await;
        let row = create(
            &pool,
            &NewDraftAttachment {
                draft_id,
                filename: "report.pdf".into(),
                content_type: "application/pdf".into(),
                content: b"PDF-bytes".to_vec(),
            },
        )
        .await
        .unwrap();
        assert_eq!(row.filename, "report.pdf");
        assert_eq!(row.size_bytes, 9);
        let listed = list_for_draft(&pool, draft_id).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, row.id);
        let bytes = load_content(&pool, row.id).await.unwrap().unwrap();
        assert_eq!(bytes, b"PDF-bytes");
    }

    #[tokio::test]
    async fn test_aggregate_size_sums_all_attachments() {
        let pool = test_pool().await;
        let draft_id = account_and_draft(&pool).await;
        for size in [10, 200, 3000] {
            create(
                &pool,
                &NewDraftAttachment {
                    draft_id,
                    filename: format!("f{size}.bin"),
                    content_type: "application/octet-stream".into(),
                    content: vec![0u8; size],
                },
            )
            .await
            .unwrap();
        }
        assert_eq!(
            aggregate_size_for_draft(&pool, draft_id).await.unwrap(),
            3210
        );
    }

    #[tokio::test]
    async fn test_aggregate_size_for_draft_with_no_rows_returns_zero() {
        let pool = test_pool().await;
        let draft_id = account_and_draft(&pool).await;
        assert_eq!(aggregate_size_for_draft(&pool, draft_id).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_delete_all_removes_rows_and_cascade_on_draft_delete() {
        let pool = test_pool().await;
        let draft_id = account_and_draft(&pool).await;
        create(
            &pool,
            &NewDraftAttachment {
                draft_id,
                filename: "a.txt".into(),
                content_type: "text/plain".into(),
                content: b"x".to_vec(),
            },
        )
        .await
        .unwrap();
        assert_eq!(delete_all_for_draft(&pool, draft_id).await.unwrap(), 1);
        assert!(list_for_draft(&pool, draft_id).await.unwrap().is_empty());

        // Cascade: re-create and delete the draft.
        create(
            &pool,
            &NewDraftAttachment {
                draft_id,
                filename: "b.txt".into(),
                content_type: "text/plain".into(),
                content: b"y".to_vec(),
            },
        )
        .await
        .unwrap();
        drafts::delete(&pool, draft_id).await.unwrap();
        assert!(list_for_draft(&pool, draft_id).await.unwrap().is_empty());
    }
}
