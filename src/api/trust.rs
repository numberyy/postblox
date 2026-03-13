use axum::extract::{Path, State};
use axum::Json;
use uuid::Uuid;

use super::auth::AuthOrg;
use super::error::ApiError;
use super::{get_inbox_for_org, AppState};
use crate::models::TrustScore;

pub async fn get(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path(inbox_id): Path<Uuid>,
) -> Result<Json<TrustScore>, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let score = crate::db::trust::get_or_create(&state.pool, inbox_id)
        .await
        .map_err(ApiError::from_sqlx)?;

    Ok(Json(score))
}
