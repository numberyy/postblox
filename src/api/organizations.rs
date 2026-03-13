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

    let org = crate::db::organizations::create(&state.pool, name)
        .await
        .map_err(ApiError::from_sqlx)?;

    let gk = generate_api_key();
    let key = crate::db::api_keys::create(
        &state.pool,
        org.id,
        &gk.key_hash,
        &gk.prefix,
        Some("default"),
    )
    .await
    .map_err(ApiError::from_sqlx)?;

    crate::db::members::ensure_admin_exists(&state.pool, org.id, key.id)
        .await
        .map_err(ApiError::from_sqlx)?;

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
