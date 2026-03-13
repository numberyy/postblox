use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::auth::AuthOrg;
use super::error::ApiError;
use super::{get_inbox_for_org, AppState};
use crate::models::Inbox;

#[derive(Deserialize)]
pub struct CreateInboxRequest {
    pub email: String,
    pub display_name: Option<String>,
    pub inbox_type: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Json(req): Json<CreateInboxRequest>,
) -> Result<(StatusCode, Json<Inbox>), ApiError> {
    if req.email.trim().is_empty() {
        return Err(ApiError::BadRequest("email is required".into()));
    }

    let inbox_type = req.inbox_type.as_deref().unwrap_or("native");

    let inbox = crate::db::inboxes::create(
        &state.pool,
        org_id,
        &req.email,
        req.display_name.as_deref(),
        inbox_type,
    )
    .await
    .map_err(ApiError::from_sqlx)?;

    if let Some(ref stalwart) = state.stalwart {
        if let Err(e) = stalwart.create_account(&inbox.email, &inbox.email).await {
            tracing::warn!("stalwart account creation failed for {}: {e}", inbox.email);
        }
    }

    Ok((StatusCode::CREATED, Json(inbox)))
}

pub async fn list(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
) -> Result<Json<Vec<Inbox>>, ApiError> {
    let inboxes = crate::db::inboxes::list_by_org(&state.pool, org_id)
        .await
        .map_err(ApiError::from_sqlx)?;

    Ok(Json(inboxes))
}

pub async fn get(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path(id): Path<Uuid>,
) -> Result<Json<Inbox>, ApiError> {
    let inbox = get_inbox_for_org(&state.pool, id, org_id).await?;
    Ok(Json(inbox))
}

pub async fn delete(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let inbox = get_inbox_for_org(&state.pool, id, org_id).await?;

    if let Some(ref stalwart) = state.stalwart {
        if let Err(e) = stalwart.delete_account(&inbox.email).await {
            tracing::warn!("stalwart account deletion failed for {}: {e}", inbox.email);
        }
    }

    crate::db::inboxes::delete(&state.pool, id)
        .await
        .map_err(ApiError::from_sqlx)?;

    Ok(StatusCode::NO_CONTENT)
}
