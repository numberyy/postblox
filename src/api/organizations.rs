use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use super::api_keys::generate_api_key;
use super::error::ApiError;
use super::AppState;

#[derive(Deserialize)]
pub struct CreateOrgRequest {
    pub name: String,
}

#[derive(Serialize)]
pub struct BootstrapResponse {
    pub organization: crate::models::Organization,
    pub api_key: String,
}

pub async fn bootstrap(
    State(state): State<AppState>,
    Json(req): Json<CreateOrgRequest>,
) -> Result<(StatusCode, Json<BootstrapResponse>), ApiError> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }

    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Advisory lock prevents concurrent bootstrap race
    sqlx::query("SELECT pg_advisory_xact_lock(42)")
        .execute(&mut *tx)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM organizations")
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if count > 0 {
        return Err(ApiError::Forbidden(
            "bootstrap disabled: organization already exists".into(),
        ));
    }

    let org: crate::models::Organization = sqlx::query_as(
        "INSERT INTO organizations (name) VALUES ($1) RETURNING id, name, created_at",
    )
    .bind(name)
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::from_sqlx)?;

    let gk = generate_api_key();
    let key: crate::models::ApiKey = sqlx::query_as(
        "INSERT INTO api_keys (org_id, key_hash, prefix, name) \
         VALUES ($1, $2, $3, $4) \
         RETURNING id, org_id, key_hash, prefix, name, created_at, last_used_at",
    )
    .bind(org.id)
    .bind(&gk.key_hash)
    .bind(&gk.prefix)
    .bind("default")
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::from_sqlx)?;

    let _: crate::models::OrgMember = sqlx::query_as(
        "INSERT INTO org_members (org_id, api_key_id, role) \
         VALUES ($1, $2, 'admin') \
         ON CONFLICT (org_id, api_key_id) DO UPDATE SET role = org_members.role \
         RETURNING id, org_id, api_key_id, role, created_at",
    )
    .bind(org.id)
    .bind(key.id)
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::from_sqlx)?;

    tx.commit()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(BootstrapResponse {
            organization: org,
            api_key: gk.full_key,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_org_request_deserialize_valid() {
        let json = r#"{"name": "My Org"}"#;
        let req: CreateOrgRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name, "My Org");
    }

    #[test]
    fn test_create_org_request_deserialize_missing_name_fails() {
        let json = r#"{}"#;
        assert!(serde_json::from_str::<CreateOrgRequest>(json).is_err());
    }

    #[test]
    fn test_bootstrap_response_serializes_correctly() {
        let resp = BootstrapResponse {
            organization: crate::models::Organization {
                id: uuid::Uuid::new_v4(),
                name: "Test".into(),
                created_at: chrono::Utc::now(),
            },
            api_key: "pb_abc12.deadbeefdeadbeefdeadbeefdeadbeef".into(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json.get("organization").is_some());
        assert!(json.get("api_key").is_some());
        assert!(json["api_key"].as_str().unwrap().starts_with("pb_"));
    }

    #[test]
    fn test_bootstrap_response_excludes_key_hash() {
        let resp = BootstrapResponse {
            organization: crate::models::Organization {
                id: uuid::Uuid::new_v4(),
                name: "Test".into(),
                created_at: chrono::Utc::now(),
            },
            api_key: "pb_abc12.deadbeef".into(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json.get("key_hash").is_none());
    }
}
