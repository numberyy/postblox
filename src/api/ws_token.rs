use std::time::{Duration, Instant};

use axum::extract::State;
use axum::Json;
use serde::Serialize;
use uuid::Uuid;

use super::auth::AuthOrg;
use super::error::ApiError;
use super::AppState;

const TOKEN_TTL_SECS: u64 = 60;
const MAX_TOKENS: usize = 10_000;

pub struct WsTokenStore {
    tokens: dashmap::DashMap<String, (Uuid, Instant)>,
}

impl Default for WsTokenStore {
    fn default() -> Self {
        Self::new()
    }
}

impl WsTokenStore {
    pub fn new() -> Self {
        Self {
            tokens: dashmap::DashMap::new(),
        }
    }

    pub fn insert(&self, token: String, org_id: Uuid, ttl: Duration) {
        if self.tokens.len() >= MAX_TOKENS {
            self.cleanup_expired();
        }
        self.tokens.insert(token, (org_id, Instant::now() + ttl));
    }

    pub fn consume(&self, token: &str) -> Option<Uuid> {
        let (_, (org_id, expires_at)) = self.tokens.remove(token)?;
        if Instant::now() > expires_at {
            return None;
        }
        Some(org_id)
    }

    pub fn cleanup_expired(&self) {
        let now = Instant::now();
        self.tokens.retain(|_, (_, expires_at)| *expires_at > now);
    }
}

#[derive(Serialize)]
pub struct WsTokenResponse {
    pub token: String,
    pub expires_at: String,
}

pub async fn create_token(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
) -> Result<Json<WsTokenResponse>, ApiError> {
    let token = generate_token();
    let ttl = Duration::from_secs(TOKEN_TTL_SECS);
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(TOKEN_TTL_SECS as i64);
    state.ws_token_store.insert(token.clone(), org_id, ttl);
    Ok(Json(WsTokenResponse {
        token,
        expires_at: expires_at.to_rfc3339(),
    }))
}

fn generate_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ws_token_store_insert_and_consume() {
        let store = WsTokenStore::new();
        let org_id = Uuid::new_v4();
        store.insert("token123".into(), org_id, Duration::from_secs(60));
        assert_eq!(store.consume("token123"), Some(org_id));
    }

    #[test]
    fn test_ws_token_single_use() {
        let store = WsTokenStore::new();
        let org_id = Uuid::new_v4();
        store.insert("token123".into(), org_id, Duration::from_secs(60));
        assert!(store.consume("token123").is_some());
        assert!(store.consume("token123").is_none());
    }

    #[test]
    fn test_ws_token_expired() {
        let store = WsTokenStore::new();
        let org_id = Uuid::new_v4();
        store.insert("token123".into(), org_id, Duration::from_secs(0));
        std::thread::sleep(Duration::from_millis(10));
        assert!(store.consume("token123").is_none());
    }

    #[test]
    fn test_ws_token_cleanup() {
        let store = WsTokenStore::new();
        store.insert("alive".into(), Uuid::new_v4(), Duration::from_secs(60));
        store.insert("dead".into(), Uuid::new_v4(), Duration::from_secs(0));
        std::thread::sleep(Duration::from_millis(10));
        store.cleanup_expired();
        assert!(store.consume("alive").is_some());
        // "dead" was cleaned up
        assert!(store.consume("dead").is_none());
    }

    #[test]
    fn test_ws_token_nonexistent() {
        let store = WsTokenStore::new();
        assert!(store.consume("nonexistent").is_none());
    }

    #[test]
    fn test_generate_token_length() {
        let token = generate_token();
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_generate_token_unique() {
        let t1 = generate_token();
        let t2 = generate_token();
        assert_ne!(t1, t2);
    }

    #[test]
    fn test_ws_token_different_orgs() {
        let store = WsTokenStore::new();
        let org1 = Uuid::new_v4();
        let org2 = Uuid::new_v4();
        store.insert("tok1".into(), org1, Duration::from_secs(60));
        store.insert("tok2".into(), org2, Duration::from_secs(60));
        assert_eq!(store.consume("tok1"), Some(org1));
        assert_eq!(store.consume("tok2"), Some(org2));
    }
}
