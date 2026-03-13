use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::auth::AuthOrg;
use super::error::ApiError;
use super::AppState;

#[derive(Deserialize)]
pub struct CreateWebhookRequest {
    pub url: String,
    pub events: Vec<String>,
}

#[derive(Serialize)]
pub struct WebhookResponse {
    pub id: Uuid,
    pub org_id: Uuid,
    pub url: String,
    pub events: serde_json::Value,
    pub active: bool,
    pub created_at: DateTime<Utc>,
}

impl From<crate::models::Webhook> for WebhookResponse {
    fn from(w: crate::models::Webhook) -> Self {
        Self {
            id: w.id,
            org_id: w.org_id,
            url: w.url,
            events: w.events,
            active: w.active,
            created_at: w.created_at,
        }
    }
}

#[derive(Serialize)]
pub struct CreateWebhookResponse {
    pub id: Uuid,
    pub org_id: Uuid,
    pub url: String,
    pub events: serde_json::Value,
    pub secret: String,
    pub active: bool,
    pub created_at: DateTime<Utc>,
}

fn generate_secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

pub async fn create(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Json(req): Json<CreateWebhookRequest>,
) -> Result<(StatusCode, Json<CreateWebhookResponse>), ApiError> {
    if req.url.trim().is_empty() {
        return Err(ApiError::BadRequest("url is required".into()));
    }

    for event in &req.events {
        if !crate::events::KNOWN_EVENTS.contains(&event.as_str()) {
            return Err(ApiError::BadRequest(format!("unknown event: {event}")));
        }
    }

    let secret = generate_secret();
    let events =
        serde_json::to_value(&req.events).map_err(|e| ApiError::Internal(e.to_string()))?;

    let wh = crate::db::webhooks::create(&state.pool, org_id, &req.url, &events, &secret)
        .await
        .map_err(ApiError::from_sqlx)?;

    Ok((
        StatusCode::CREATED,
        Json(CreateWebhookResponse {
            id: wh.id,
            org_id: wh.org_id,
            url: wh.url,
            events: wh.events,
            secret: wh.secret,
            active: wh.active,
            created_at: wh.created_at,
        }),
    ))
}

pub async fn list(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
) -> Result<Json<Vec<WebhookResponse>>, ApiError> {
    let webhooks = crate::db::webhooks::list_by_org(&state.pool, org_id)
        .await
        .map_err(ApiError::from_sqlx)?;

    Ok(Json(
        webhooks.into_iter().map(WebhookResponse::from).collect(),
    ))
}

pub async fn get(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path(id): Path<Uuid>,
) -> Result<Json<WebhookResponse>, ApiError> {
    let wh = get_webhook_for_org(&state.pool, id, org_id).await?;
    Ok(Json(WebhookResponse::from(wh)))
}

pub async fn delete(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let wh = get_webhook_for_org(&state.pool, id, org_id).await?;

    crate::db::webhooks::delete(&state.pool, wh.id)
        .await
        .map_err(ApiError::from_sqlx)?;

    Ok(StatusCode::NO_CONTENT)
}

async fn get_webhook_for_org(
    pool: &sqlx::PgPool,
    id: Uuid,
    org_id: Uuid,
) -> Result<crate::models::Webhook, ApiError> {
    let wh = crate::db::webhooks::get_by_id(pool, id)
        .await
        .map_err(ApiError::from_sqlx)?
        .ok_or(ApiError::NotFound)?;
    if wh.org_id != org_id {
        return Err(ApiError::NotFound);
    }
    Ok(wh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_secret_is_64_hex_chars() {
        let secret = generate_secret();
        assert_eq!(secret.len(), 64);
        assert!(secret.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_generate_secret_is_unique() {
        let s1 = generate_secret();
        let s2 = generate_secret();
        assert_ne!(s1, s2);
    }

    #[test]
    fn test_webhook_response_excludes_secret() {
        let wh = crate::models::Webhook {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            url: "https://example.com".into(),
            events: serde_json::json!(["msg.in"]),
            secret: "supersecret".into(),
            active: true,
            created_at: Utc::now(),
        };

        let resp = WebhookResponse::from(wh);
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json.get("secret").is_none());
    }

    #[test]
    fn test_create_webhook_response_includes_secret() {
        let resp = CreateWebhookResponse {
            id: Uuid::new_v4(),
            org_id: Uuid::new_v4(),
            url: "https://example.com".into(),
            events: serde_json::json!(["msg.in"]),
            secret: "the_secret".into(),
            active: true,
            created_at: Utc::now(),
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["secret"], "the_secret");
    }
}
