use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::auth::AuthOrg;
use super::error::ApiError;
use super::{get_inbox_for_org, AppState};
use crate::models::Label;

#[derive(Deserialize)]
pub struct CreateLabelRequest {
    pub name: String,
    pub color: Option<String>,
}

#[derive(Deserialize)]
pub struct AddLabelRequest {
    pub label_id: Uuid,
}

pub async fn create(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path(inbox_id): Path<Uuid>,
    Json(req): Json<CreateLabelRequest>,
) -> Result<(StatusCode, Json<Label>), ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }

    let label = crate::db::labels::create(&state.pool, inbox_id, &req.name, req.color.as_deref())
        .await
        .map_err(ApiError::from_sqlx)?;

    Ok((StatusCode::CREATED, Json(label)))
}

pub async fn list(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path(inbox_id): Path<Uuid>,
) -> Result<Json<Vec<Label>>, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let labels = crate::db::labels::list_by_inbox(&state.pool, inbox_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(labels))
}

pub async fn delete(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path((inbox_id, id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let label = crate::db::labels::get_by_id(&state.pool, id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound)?;

    if label.inbox_id != inbox_id {
        return Err(ApiError::NotFound);
    }

    crate::db::labels::delete(&state.pool, id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn add_to_message(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path((inbox_id, message_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<AddLabelRequest>,
) -> Result<StatusCode, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let msg = crate::db::messages::get_by_id(&state.pool, message_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound)?;
    if msg.inbox_id != inbox_id {
        return Err(ApiError::NotFound);
    }

    let label = crate::db::labels::get_by_id(&state.pool, req.label_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound)?;
    if label.inbox_id != inbox_id {
        return Err(ApiError::NotFound);
    }

    crate::db::labels::add_to_message(&state.pool, message_id, req.label_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_from_message(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path((inbox_id, message_id, label_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let msg = crate::db::messages::get_by_id(&state.pool, message_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound)?;
    if msg.inbox_id != inbox_id {
        return Err(ApiError::NotFound);
    }

    let label = crate::db::labels::get_by_id(&state.pool, label_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound)?;
    if label.inbox_id != inbox_id {
        return Err(ApiError::NotFound);
    }

    crate::db::labels::remove_from_message(&state.pool, message_id, label_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_for_message(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path((inbox_id, message_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<Label>>, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let msg = crate::db::messages::get_by_id(&state.pool, message_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound)?;
    if msg.inbox_id != inbox_id {
        return Err(ApiError::NotFound);
    }

    let labels = crate::db::labels::list_for_message(&state.pool, message_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(labels))
}
