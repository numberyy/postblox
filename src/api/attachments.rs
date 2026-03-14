use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::Json;
use uuid::Uuid;

use super::auth::AuthOrg;
use super::error::ApiError;
use super::{get_inbox_for_org, AppState};
use crate::models::Attachment;

pub async fn list(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path((inbox_id, message_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<Attachment>>, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let msg = crate::db::messages::get_by_id(&state.pool, message_id)
        .await
        .map_err(ApiError::from_sqlx)?
        .ok_or(ApiError::NotFound)?;
    if msg.inbox_id != inbox_id {
        return Err(ApiError::NotFound);
    }

    let attachments = crate::db::attachments::list_by_message(&state.pool, message_id)
        .await
        .map_err(ApiError::from_sqlx)?;

    Ok(Json(attachments))
}

pub async fn download(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path((inbox_id, message_id, attachment_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<Response, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let msg = crate::db::messages::get_by_id(&state.pool, message_id)
        .await
        .map_err(ApiError::from_sqlx)?
        .ok_or(ApiError::NotFound)?;
    if msg.inbox_id != inbox_id {
        return Err(ApiError::NotFound);
    }

    let attachment = crate::db::attachments::get_by_id(&state.pool, attachment_id)
        .await
        .map_err(ApiError::from_sqlx)?
        .ok_or(ApiError::NotFound)?;
    if attachment.message_id != message_id {
        return Err(ApiError::NotFound);
    }

    let data =
        crate::storage::read_attachment(&state.attachment_storage_path, &attachment.storage_key)
            .await
            .map_err(|e| ApiError::Internal(format!("failed to read attachment: {e}")))?;

    let disposition = format!(
        "{}; filename=\"{}\"",
        attachment.disposition,
        attachment.filename.replace('"', "\\\"")
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, &attachment.content_type)
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(header::CONTENT_LENGTH, data.len())
        .body(Body::from(data))
        .map_err(|e| ApiError::Internal(format!("failed to build attachment response: {e}")))
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_content_disposition_escapes_quotes() {
        let filename = r#"file"name.txt"#;
        let disposition = format!("attachment; filename=\"{}\"", filename.replace('"', "\\\""));
        assert_eq!(disposition, r#"attachment; filename="file\"name.txt""#);
    }

    #[test]
    fn test_content_disposition_normal_filename() {
        let filename = "report.pdf";
        let disposition = format!("attachment; filename=\"{}\"", filename.replace('"', "\\\""));
        assert_eq!(disposition, "attachment; filename=\"report.pdf\"");
    }
}
