use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::auth::AdminOrg;
use super::error::ApiError;
use super::AppState;
use crate::models::Role;

#[derive(Deserialize)]
pub struct AddMemberRequest {
    pub api_key_id: Uuid,
    pub role: Role,
}

#[derive(Serialize)]
pub struct MemberResponse {
    pub id: Uuid,
    pub org_id: Uuid,
    pub api_key_id: Uuid,
    pub role: Role,
    pub created_at: DateTime<Utc>,
}

impl From<crate::models::OrgMember> for MemberResponse {
    fn from(m: crate::models::OrgMember) -> Self {
        Self {
            id: m.id,
            org_id: m.org_id,
            api_key_id: m.api_key_id,
            role: m.role,
            created_at: m.created_at,
        }
    }
}

pub async fn list(
    State(state): State<AppState>,
    AdminOrg(org_id): AdminOrg,
    Query(params): Query<super::PaginationParams>,
) -> Result<Json<Vec<MemberResponse>>, ApiError> {
    let (limit, offset) = super::clamp_pagination(&params);
    let members = crate::db::members::list_by_org(&state.pool, org_id, limit, offset)
        .await
        .map_err(ApiError::from_sqlx)?;
    Ok(Json(
        members.into_iter().map(MemberResponse::from).collect(),
    ))
}

pub async fn add(
    State(state): State<AppState>,
    AdminOrg(org_id): AdminOrg,
    Json(req): Json<AddMemberRequest>,
) -> Result<(StatusCode, Json<MemberResponse>), ApiError> {
    let (exists,): (bool,) =
        sqlx::query_as("SELECT EXISTS(SELECT 1 FROM api_keys WHERE id = $1 AND org_id = $2)")
            .bind(req.api_key_id)
            .bind(org_id)
            .fetch_one(&state.pool)
            .await
            .map_err(ApiError::from_sqlx)?;
    if !exists {
        return Err(ApiError::NotFound);
    }

    let member = crate::db::members::create(&state.pool, org_id, req.api_key_id, req.role)
        .await
        .map_err(ApiError::from_sqlx)?;
    Ok((StatusCode::CREATED, Json(MemberResponse::from(member))))
}

pub async fn remove(
    State(state): State<AppState>,
    AdminOrg(org_id): AdminOrg,
    Path(api_key_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    match crate::db::members::delete_unless_last_admin(&state.pool, org_id, api_key_id).await {
        Ok(true) => Ok(StatusCode::NO_CONTENT),
        Ok(false) => Err(ApiError::NotFound),
        Err(crate::db::members::MemberError::LastAdmin) => {
            Err(ApiError::BadRequest("cannot remove the last admin".into()))
        }
        Err(crate::db::members::MemberError::Db(e)) => Err(ApiError::from_sqlx(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_member_request_deserialize() {
        let json = serde_json::json!({
            "api_key_id": Uuid::new_v4(),
            "role": "member"
        });
        let req: AddMemberRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.role, Role::Member);
    }

    #[test]
    fn test_add_member_request_admin_role() {
        let json = serde_json::json!({
            "api_key_id": Uuid::new_v4(),
            "role": "admin"
        });
        let req: AddMemberRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.role, Role::Admin);
    }

    #[test]
    fn test_add_member_request_invalid_role_fails() {
        let json = serde_json::json!({
            "api_key_id": Uuid::new_v4(),
            "role": "superadmin"
        });
        assert!(serde_json::from_value::<AddMemberRequest>(json).is_err());
    }

    #[test]
    fn test_member_response_from_org_member() {
        let member = crate::models::OrgMember {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            api_key_id: Uuid::new_v4(),
            role: Role::Admin,
            created_at: Utc::now(),
        };
        let resp = MemberResponse::from(member.clone());
        assert_eq!(resp.id, member.id);
        assert_eq!(resp.role, Role::Admin);
    }

    #[test]
    fn test_member_response_serialization() {
        let resp = MemberResponse {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            api_key_id: Uuid::new_v4(),
            role: Role::Member,
            created_at: Utc::now(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["role"], "member");
        assert!(json.get("id").is_some());
    }
}
