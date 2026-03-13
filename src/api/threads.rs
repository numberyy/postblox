use axum::extract::{Path, Query, State};
use axum::Json;
use uuid::Uuid;

use super::auth::AuthOrg;
use super::error::ApiError;
use super::{clamp_pagination, get_inbox_for_org, AppState, PaginationParams};
use crate::models::Thread;

pub async fn list(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path(inbox_id): Path<Uuid>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<Vec<Thread>>, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let (limit, offset) = clamp_pagination(&params);

    let threads = crate::db::threads::list_by_inbox(&state.pool, inbox_id, limit, offset)
        .await
        .map_err(ApiError::from_sqlx)?;

    Ok(Json(threads))
}

pub async fn get(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path((inbox_id, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Thread>, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let thread = crate::db::threads::get_by_id(&state.pool, id)
        .await
        .map_err(ApiError::from_sqlx)?
        .ok_or(ApiError::NotFound)?;

    if thread.inbox_id != inbox_id {
        return Err(ApiError::NotFound);
    }

    Ok(Json(thread))
}

#[cfg(test)]
mod tests {
    use super::super::{clamp_pagination, PaginationParams};

    #[test]
    fn test_clamp_pagination_defaults() {
        let params = PaginationParams {
            limit: None,
            offset: None,
        };
        let (limit, offset) = clamp_pagination(&params);
        assert_eq!(limit, 50);
        assert_eq!(offset, 0);
    }

    #[test]
    fn test_clamp_pagination_caps_at_100() {
        let params = PaginationParams {
            limit: Some(999),
            offset: None,
        };
        let (limit, _) = clamp_pagination(&params);
        assert_eq!(limit, 100);
    }

    #[test]
    fn test_clamp_pagination_zero_limit_becomes_1() {
        let params = PaginationParams {
            limit: Some(0),
            offset: None,
        };
        let (limit, _) = clamp_pagination(&params);
        assert_eq!(limit, 1);
    }

    #[test]
    fn test_clamp_pagination_negative_offset_becomes_0() {
        let params = PaginationParams {
            limit: None,
            offset: Some(-5),
        };
        let (_, offset) = clamp_pagination(&params);
        assert_eq!(offset, 0);
    }

    #[test]
    fn test_clamp_pagination_valid_values_unchanged() {
        let params = PaginationParams {
            limit: Some(25),
            offset: Some(10),
        };
        let (limit, offset) = clamp_pagination(&params);
        assert_eq!(limit, 25);
        assert_eq!(offset, 10);
    }

    #[test]
    fn test_clamp_pagination_negative_limit_becomes_1() {
        let params = PaginationParams {
            limit: Some(-10),
            offset: None,
        };
        let (limit, _) = clamp_pagination(&params);
        assert_eq!(limit, 1);
    }
}
