use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::auth::AuthOrg;
use super::error::ApiError;
use super::AppState;
use crate::models::{NotificationConfig, NotificationProvider};

#[derive(Deserialize)]
pub struct CreateNotificationRequest {
    pub provider: NotificationProvider,
    pub config: serde_json::Value,
}

pub async fn list(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
) -> Result<Json<Vec<NotificationConfig>>, ApiError> {
    let configs = crate::db::notifications::list_active(&state.pool, org_id)
        .await
        .map_err(ApiError::from_sqlx)?;

    Ok(Json(configs))
}

pub async fn create(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Json(req): Json<CreateNotificationRequest>,
) -> Result<(StatusCode, Json<NotificationConfig>), ApiError> {
    let input = crate::models::CreateNotificationConfig {
        org_id,
        provider: req.provider,
        config: req.config,
    };

    let nc = crate::db::notifications::create(&state.pool, &input)
        .await
        .map_err(ApiError::from_sqlx)?;

    Ok((StatusCode::CREATED, Json(nc)))
}

pub async fn delete(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let deleted = crate::db::notifications::delete(&state.pool, id, org_id)
        .await
        .map_err(ApiError::from_sqlx)?;

    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_notification_request_deserialize_ntfy() {
        let json = serde_json::json!({
            "provider": "ntfy",
            "config": {"url": "https://ntfy.sh/postblox"}
        });
        let req: CreateNotificationRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.provider, NotificationProvider::Ntfy);
        assert_eq!(req.config["url"], "https://ntfy.sh/postblox");
    }

    #[test]
    fn test_create_notification_request_deserialize_email() {
        let json = serde_json::json!({
            "provider": "email",
            "config": {"to": "admin@example.com"}
        });
        let req: CreateNotificationRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.provider, NotificationProvider::Email);
    }

    #[test]
    fn test_create_notification_request_deserialize_webhook() {
        let json = serde_json::json!({
            "provider": "webhook",
            "config": {"url": "https://example.com/hook"}
        });
        let req: CreateNotificationRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.provider, NotificationProvider::Webhook);
    }

    #[test]
    fn test_create_notification_request_deserialize_desktop() {
        let json = serde_json::json!({
            "provider": "desktop",
            "config": {}
        });
        let req: CreateNotificationRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.provider, NotificationProvider::Desktop);
    }

    #[test]
    fn test_create_notification_request_invalid_provider_fails() {
        let json = serde_json::json!({
            "provider": "slack",
            "config": {}
        });
        let result = serde_json::from_value::<CreateNotificationRequest>(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_create_notification_request_missing_config_fails() {
        let json = serde_json::json!({
            "provider": "ntfy"
        });
        let result = serde_json::from_value::<CreateNotificationRequest>(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_create_notification_request_empty_config() {
        let json = serde_json::json!({
            "provider": "ntfy",
            "config": {}
        });
        let req: CreateNotificationRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.config, serde_json::json!({}));
    }
}
