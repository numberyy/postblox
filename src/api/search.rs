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
    pub semantic: Option<bool>,
    pub threshold: Option<f64>,
}

pub async fn search(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<Message>>, ApiError> {
    if params.q.trim().is_empty() {
        return Err(ApiError::BadRequest("search query required".into()));
    }

    let pagination = super::PaginationParams {
        limit: params.limit,
        offset: params.offset,
    };
    let (limit, offset) = super::clamp_pagination(&pagination);

    if params.semantic.unwrap_or(false) {
        let provider = state
            .embedding_provider
            .as_ref()
            .ok_or_else(|| ApiError::BadRequest("semantic search not configured".into()))?;

        let embedding = provider
            .embed(&params.q)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;

        let threshold = params.threshold.unwrap_or(0.7).clamp(0.0, 1.0);
        let results = crate::db::embeddings::search_similar(
            &state.pool,
            org_id,
            &embedding,
            limit,
            threshold,
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

        return Ok(Json(results));
    }

    let results = crate::db::messages::search(&state.pool, org_id, &params.q, limit, offset)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(results))
}
