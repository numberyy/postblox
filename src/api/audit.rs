use axum::extract::{Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use uuid::Uuid;

use super::auth::AuthOrg;
use super::error::ApiError;
use super::AppState;
use crate::models::AuditEntry;

#[derive(Deserialize)]
pub struct AuditQueryParams {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub inbox_id: Option<Uuid>,
    pub action: Option<String>,
    pub after: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
}

pub async fn list(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Query(params): Query<AuditQueryParams>,
) -> Result<Json<Vec<AuditEntry>>, ApiError> {
    let limit = params.limit.unwrap_or(50).clamp(1, 100);
    let offset = params.offset.unwrap_or(0).max(0);

    let entries = crate::db::audit::list_entries(
        &state.pool,
        org_id,
        offset,
        limit,
        params.inbox_id,
        params.action.as_deref(),
        params.after,
        params.before,
    )
    .await
    .map_err(ApiError::from_sqlx)?;

    Ok(Json(entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_query_params_all_fields() {
        let json = serde_json::json!({
            "limit": 10,
            "offset": 5,
            "inbox_id": "00000000-0000-0000-0000-000000000001",
            "action": "message_sent",
            "after": "2026-01-01T00:00:00Z",
            "before": "2026-12-31T23:59:59Z"
        });
        let params: AuditQueryParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.limit, Some(10));
        assert_eq!(params.offset, Some(5));
        assert!(params.inbox_id.is_some());
        assert_eq!(params.action.as_deref(), Some("message_sent"));
        assert!(params.after.is_some());
        assert!(params.before.is_some());
    }

    #[test]
    fn test_audit_query_params_all_optional() {
        let json = serde_json::json!({});
        let params: AuditQueryParams = serde_json::from_value(json).unwrap();
        assert!(params.limit.is_none());
        assert!(params.offset.is_none());
        assert!(params.inbox_id.is_none());
        assert!(params.action.is_none());
        assert!(params.after.is_none());
        assert!(params.before.is_none());
    }
}
