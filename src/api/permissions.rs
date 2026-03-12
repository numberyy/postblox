use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::auth::AuthOrg;
use super::error::ApiError;
use super::{get_inbox_for_org, AppState};
use crate::models::{Permission, SendMode};

#[derive(Deserialize)]
pub struct UpsertPermissionRequest {
    pub send_mode: Option<SendMode>,
}

pub async fn get(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path(inbox_id): Path<Uuid>,
) -> Result<Json<Permission>, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let perm = crate::db::permissions::get_by_inbox(&state.pool, inbox_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound)?;

    Ok(Json(perm))
}

pub async fn upsert(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path(inbox_id): Path<Uuid>,
    Json(req): Json<UpsertPermissionRequest>,
) -> Result<Json<Permission>, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let mode = req.send_mode.unwrap_or_default();

    let perm = crate::db::permissions::upsert(&state.pool, inbox_id, mode)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(perm))
}
