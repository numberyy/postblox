use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::models::Role;

use super::error::ApiError;
use super::AppState;

pub struct AuthOrg {
    pub org_id: Uuid,
    pub role: Role,
}

impl FromRequestParts<AppState> for AuthOrg {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = extract_bearer_token(parts)?;
        let auth_info = match validate_api_key(&state.pool, &token).await {
            Ok(info) => info,
            Err(AuthError::Invalid) => return Err(ApiError::Unauthorized),
            Err(AuthError::DatabaseError) => {
                return Err(ApiError::Internal(
                    "authentication service unavailable".into(),
                ))
            }
        };

        // Best-effort update; auth must not fail if this write fails.
        let pool = state.pool.clone();
        let key_id = auth_info.id;
        tokio::spawn(async move {
            if let Err(e) = crate::db::api_keys::touch_last_used(&pool, key_id).await {
                tracing::debug!("failed to touch last_used: {e}");
            }
        });

        Ok(AuthOrg {
            org_id: auth_info.org_id,
            // Default to Member (least privilege) for legacy keys without explicit role
            role: auth_info.role.unwrap_or(Role::Member),
        })
    }
}

pub struct AdminOrg(pub Uuid);

impl FromRequestParts<AppState> for AdminOrg {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth = AuthOrg::from_request_parts(parts, state).await?;
        if auth.role != Role::Admin {
            return Err(ApiError::Forbidden("admin role required".into()));
        }
        Ok(AdminOrg(auth.org_id))
    }
}

fn extract_bearer_token(parts: &Parts) -> Result<String, ApiError> {
    let header = parts
        .headers
        .get("authorization")
        .ok_or(ApiError::Unauthorized)?
        .to_str()
        .map_err(|_| ApiError::Unauthorized)?;

    let token = header
        .strip_prefix("Bearer ")
        .ok_or(ApiError::Unauthorized)?;

    if token.is_empty() {
        return Err(ApiError::Unauthorized);
    }

    Ok(token.to_string())
}

pub enum AuthError {
    Invalid,
    DatabaseError,
}

pub async fn validate_api_key(
    pool: &sqlx::PgPool,
    key: &str,
) -> Result<crate::db::api_keys::AuthKeyInfo, AuthError> {
    if key.len() < 8 || !key.starts_with("pb_") {
        return Err(AuthError::Invalid);
    }
    let prefix = &key[..8];
    let stored = match crate::db::api_keys::find_by_prefix_with_role(pool, prefix).await {
        Ok(Some(s)) => s,
        Ok(None) => return Err(AuthError::Invalid),
        Err(e) => {
            tracing::error!("database error during API key validation: {e}");
            return Err(AuthError::DatabaseError);
        }
    };
    let token_hash = hash_key(key);
    if !constant_time_eq(token_hash.as_bytes(), stored.key_hash.as_bytes()) {
        return Err(AuthError::Invalid);
    }
    Ok(stored)
}

#[must_use]
pub fn hash_key(key: &str) -> String {
    let hash = Sha256::digest(key.as_bytes());
    format!("{hash:x}")
}

#[must_use]
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    fn parts_with_auth(value: &str) -> Parts {
        let (parts, _) = Request::builder()
            .header("authorization", value)
            .body(())
            .unwrap()
            .into_parts();
        parts
    }

    fn parts_no_headers() -> Parts {
        let (parts, _) = Request::builder().body(()).unwrap().into_parts();
        parts
    }

    #[test]
    fn test_extract_bearer_token_valid() {
        let parts = parts_with_auth("Bearer pb_test1234abcdef");
        assert_eq!(extract_bearer_token(&parts).unwrap(), "pb_test1234abcdef");
    }

    #[test]
    fn test_extract_bearer_token_missing_header() {
        let parts = parts_no_headers();
        assert!(extract_bearer_token(&parts).is_err());
    }

    #[test]
    fn test_extract_bearer_token_not_bearer() {
        let parts = parts_with_auth("Basic abc123");
        assert!(extract_bearer_token(&parts).is_err());
    }

    #[test]
    fn test_extract_bearer_token_empty_token() {
        let parts = parts_with_auth("Bearer ");
        assert!(extract_bearer_token(&parts).is_err());
    }

    #[test]
    fn test_extract_bearer_token_missing_space_after_bearer() {
        let parts = parts_with_auth("Bearerpb_test1234");
        assert!(extract_bearer_token(&parts).is_err());
    }

    #[test]
    fn test_hash_key_deterministic() {
        let h1 = hash_key("pb_test1234");
        let h2 = hash_key("pb_test1234");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_key_different_inputs_differ() {
        assert_ne!(hash_key("pb_aaaa1111"), hash_key("pb_bbbb2222"));
    }

    #[test]
    fn test_hash_key_length_is_64_hex_chars() {
        let h = hash_key("pb_anything");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_constant_time_eq_same_values() {
        assert!(constant_time_eq(b"abcdef", b"abcdef"));
    }

    #[test]
    fn test_constant_time_eq_different_values() {
        assert!(!constant_time_eq(b"abcdef", b"abcdeg"));
    }

    #[test]
    fn test_constant_time_eq_different_lengths() {
        assert!(!constant_time_eq(b"short", b"longer_string"));
    }
}
