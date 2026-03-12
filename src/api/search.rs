use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;

use super::auth::AuthOrg;
use super::error::ApiError;
use super::AppState;
use crate::models::Message;

#[derive(Deserialize)]
pub struct SearchParams {
    pub q: String,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

pub async fn search(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<Message>>, ApiError> {
    if params.q.trim().is_empty() {
        return Err(ApiError::BadRequest("search query required".into()));
    }

    let limit = params.limit.unwrap_or(50).clamp(1, 100);
    let offset = params.offset.unwrap_or(0).max(0);

    let results = crate::db::messages::search(&state.pool, org_id, &params.q, limit, offset)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(results))
}
