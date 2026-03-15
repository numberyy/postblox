use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::auth::AdminOrg;
use super::error::ApiError;
use super::AppState;

#[derive(Deserialize)]
pub struct CreateKeyRequest {
    pub name: Option<String>,
}

#[derive(Serialize)]
pub struct CreateKeyResponse {
    pub id: Uuid,
    pub prefix: String,
    pub name: Option<String>,
    pub api_key: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct KeyResponse {
    pub id: Uuid,
    pub prefix: String,
    pub name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

impl From<crate::models::ApiKey> for KeyResponse {
    fn from(k: crate::models::ApiKey) -> Self {
        Self {
            id: k.id,
            prefix: k.prefix,
            name: k.name,
            created_at: k.created_at,
            last_used_at: k.last_used_at,
        }
    }
}

pub(crate) struct GeneratedKey {
    pub full_key: String,
    pub key_hash: String,
    pub prefix: String,
}

pub(crate) fn generate_api_key() -> GeneratedKey {
    let uuid_hex = Uuid::new_v4().simple().to_string();
    let prefix = format!("pb_{}", &uuid_hex[..5]);
    let secret = Uuid::new_v4().simple().to_string();
    let full_key = format!("{prefix}.{secret}");
    let key_hash = crate::api::auth::hash_key(&full_key);
    GeneratedKey {
        full_key,
        key_hash,
        prefix,
    }
}

pub async fn create(
    State(state): State<AppState>,
    AdminOrg(org_id): AdminOrg,
    Json(req): Json<CreateKeyRequest>,
) -> Result<(StatusCode, Json<CreateKeyResponse>), ApiError> {
    let gk = generate_api_key();

    let mut tx = state.pool.begin().await.map_err(ApiError::from_sqlx)?;

    let key: crate::models::ApiKey = sqlx::query_as(
        "INSERT INTO api_keys (org_id, key_hash, prefix, name) \
         VALUES ($1, $2, $3, $4) \
         RETURNING id, org_id, key_hash, prefix, name, created_at, last_used_at",
    )
    .bind(org_id)
    .bind(&gk.key_hash)
    .bind(&gk.prefix)
    .bind(req.name.as_deref())
    .fetch_one(&mut *tx)
    .await
    .map_err(ApiError::from_sqlx)?;

    sqlx::query("INSERT INTO org_members (org_id, api_key_id, role) VALUES ($1, $2, 'member')")
        .bind(org_id)
        .bind(key.id)
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from_sqlx)?;

    tx.commit().await.map_err(ApiError::from_sqlx)?;

    let pool = state.pool.clone();
    let key_id = key.id;
    let key_prefix = key.prefix.clone();
    let key_name = key.name.clone();
    tokio::spawn(async move {
        crate::events::audit(
            &pool,
            org_id,
            None,
            crate::models::AuditAction::ApiKeyCreated,
            "api",
            serde_json::json!({"key_id": key_id.to_string(), "prefix": key_prefix, "name": key_name}),
        )
        .await;
    });

    Ok((
        StatusCode::CREATED,
        Json(CreateKeyResponse {
            id: key.id,
            prefix: key.prefix,
            name: key.name,
            api_key: gk.full_key,
            created_at: key.created_at,
        }),
    ))
}

pub async fn list(
    State(state): State<AppState>,
    AdminOrg(org_id): AdminOrg,
) -> Result<Json<Vec<KeyResponse>>, ApiError> {
    let keys = crate::db::api_keys::list_by_org(&state.pool, org_id)
        .await
        .map_err(ApiError::from_sqlx)?;

    Ok(Json(keys.into_iter().map(KeyResponse::from).collect()))
}

pub async fn delete(
    State(state): State<AppState>,
    AdminOrg(org_id): AdminOrg,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let mut tx = state.pool.begin().await.map_err(ApiError::from_sqlx)?;

    let role: Option<(crate::models::Role,)> = sqlx::query_as(
        "SELECT role FROM org_members WHERE org_id = $1 AND api_key_id = $2 FOR UPDATE",
    )
    .bind(org_id)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(ApiError::from_sqlx)?;

    if let Some((crate::models::Role::Admin,)) = role {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM org_members WHERE org_id = $1 AND role = 'admin'")
                .bind(org_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(ApiError::from_sqlx)?;
        if count <= 1 {
            return Err(ApiError::BadRequest(
                "cannot delete the last admin key".into(),
            ));
        }
    }

    let result = sqlx::query("DELETE FROM api_keys WHERE id = $1 AND org_id = $2")
        .bind(id)
        .bind(org_id)
        .execute(&mut *tx)
        .await
        .map_err(ApiError::from_sqlx)?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    tx.commit().await.map_err(ApiError::from_sqlx)?;

    let pool = state.pool.clone();
    tokio::spawn(async move {
        crate::events::audit(
            &pool,
            org_id,
            None,
            crate::models::AuditAction::ApiKeyDeleted,
            "api",
            serde_json::json!({"key_id": id.to_string()}),
        )
        .await;
    });

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_api_key_format() {
        let gk = generate_api_key();
        assert!(gk.full_key.starts_with("pb_"));
        assert!(gk.full_key.contains('.'));
        assert_eq!(gk.prefix.len(), 8);
        assert_eq!(&gk.full_key[..8], gk.prefix);
        assert_eq!(gk.key_hash.len(), 64);
        assert!(gk.key_hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_generate_api_key_unique() {
        let k1 = generate_api_key();
        let k2 = generate_api_key();
        assert_ne!(k1.full_key, k2.full_key);
    }

    #[test]
    fn test_generate_api_key_hash_matches() {
        let gk = generate_api_key();
        let recomputed = crate::api::auth::hash_key(&gk.full_key);
        assert_eq!(gk.key_hash, recomputed);
    }

    #[test]
    fn test_generate_api_key_secret_is_32_hex() {
        let gk = generate_api_key();
        let secret = gk.full_key.split('.').nth(1).unwrap();
        assert_eq!(secret.len(), 32);
        assert!(secret.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_key_response_excludes_hash_and_org() {
        let key = crate::models::ApiKey {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            key_hash: "should_not_appear".into(),
            prefix: "pb_abc12".into(),
            name: Some("test key".into()),
            created_at: Utc::now(),
            last_used_at: None,
        };
        let resp = KeyResponse::from(key);
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json.get("key_hash").is_none());
        assert!(json.get("org_id").is_none());
        assert_eq!(json["prefix"], "pb_abc12");
    }

    #[test]
    fn test_create_key_request_name_optional() {
        let json = r#"{}"#;
        let req: CreateKeyRequest = serde_json::from_str(json).unwrap();
        assert!(req.name.is_none());
    }

    #[test]
    fn test_create_key_request_with_name() {
        let json = r#"{"name": "agent-key"}"#;
        let req: CreateKeyRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.name.as_deref(), Some("agent-key"));
    }

    #[test]
    fn test_create_key_response_includes_api_key() {
        let resp = CreateKeyResponse {
            id: Uuid::new_v4(),
            prefix: "pb_abc12".into(),
            name: None,
            api_key: "pb_abc12.deadbeefdeadbeefdeadbeefdeadbeef".into(),
            created_at: Utc::now(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["api_key"].as_str().unwrap().starts_with("pb_"));
        assert!(json.get("key_hash").is_none());
    }
}
