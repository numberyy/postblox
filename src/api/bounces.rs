use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::auth::AuthOrg;
use super::error::ApiError;
use super::{get_inbox_for_org, AppState};
use crate::models::{BounceType, DeliveryStatus, DeliveryStatusType};

pub async fn get_delivery_status(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path((inbox_id, message_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<DeliveryStatus>>, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let statuses = crate::db::bounces::get_by_message(&state.pool, message_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(statuses))
}

#[derive(Deserialize)]
pub struct BounceNotification {
    pub message_id: Uuid,
    pub status: DeliveryStatusType,
    pub bounce_type: Option<BounceType>,
    pub details: Option<serde_json::Value>,
}

const AUTO_DISABLE_THRESHOLD: i64 = 5;

pub async fn receive_bounce(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<BounceNotification>,
) -> Result<StatusCode, ApiError> {
    if let Some(ref expected) = state.inbound_token {
        let provided = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        match provided {
            Some(token)
                if crate::api::auth::constant_time_eq(token.as_bytes(), expected.as_bytes()) => {}
            _ => return Err(ApiError::Unauthorized),
        }
    }
    let msg = crate::db::messages::get_by_id(&state.pool, req.message_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound)?;

    let status_str = req.status.to_string();
    let bounce_type_str = req.bounce_type.map(|bt| bt.to_string());

    let (create_result, inbox_result) = tokio::join!(
        crate::db::bounces::create_status(
            &state.pool,
            req.message_id,
            &status_str,
            bounce_type_str.as_deref(),
            req.details,
        ),
        crate::db::inboxes::get_by_id(&state.pool, msg.inbox_id),
    );

    create_result.map_err(|e| ApiError::Internal(e.to_string()))?;
    let inbox = inbox_result
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound)?;

    let event = match req.status {
        DeliveryStatusType::Bounced | DeliveryStatusType::Complained => {
            crate::events::PostbloxEvent::MessageBounced {
                message_id: req.message_id,
                inbox_id: msg.inbox_id,
            }
        }
        DeliveryStatusType::Delivered => crate::events::PostbloxEvent::MessageDelivered {
            message_id: req.message_id,
            inbox_id: msg.inbox_id,
        },
    };

    let pool = state.pool.clone();
    let webhook_client = state.webhook_client.clone();
    let hooks = state.hooks.clone();
    let ws_hub = state.ws_hub.clone();
    let org_id = inbox.org_id;
    tokio::spawn(async move {
        crate::events::dispatch(&pool, org_id, event, &webhook_client, &hooks, &ws_hub).await;
    });

    if req.status == DeliveryStatusType::Bounced && req.bounce_type == Some(BounceType::Hard) {
        match crate::db::bounces::count_hard_bounces_for_inbox(&state.pool, msg.inbox_id).await {
            Ok(n) if n >= AUTO_DISABLE_THRESHOLD => {
                if let Err(e) =
                    crate::db::inboxes::set_active(&state.pool, msg.inbox_id, false).await
                {
                    tracing::error!(inbox_id = %msg.inbox_id, "failed to auto-disable inbox: {e}");
                }
                tracing::warn!(
                    inbox_id = %msg.inbox_id,
                    hard_bounces = n,
                    "inbox auto-disabled due to excessive hard bounces"
                );
            }
            Ok(_) => {}
            Err(e) => {
                tracing::error!(inbox_id = %msg.inbox_id, "failed to count hard bounces: {e}");
            }
        }
    }

    Ok(StatusCode::OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bounce_notification_deserialize() {
        let json = serde_json::json!({
            "message_id": Uuid::new_v4(),
            "status": "bounced",
            "bounce_type": "hard",
            "details": {"smtp_code": 550}
        });
        let bn: BounceNotification = serde_json::from_value(json).unwrap();
        assert_eq!(bn.status, DeliveryStatusType::Bounced);
        assert_eq!(bn.bounce_type, Some(BounceType::Hard));
    }

    #[test]
    fn test_bounce_notification_minimal() {
        let json = serde_json::json!({
            "message_id": Uuid::new_v4(),
            "status": "delivered"
        });
        let bn: BounceNotification = serde_json::from_value(json).unwrap();
        assert_eq!(bn.status, DeliveryStatusType::Delivered);
        assert!(bn.bounce_type.is_none());
        assert!(bn.details.is_none());
    }

    #[test]
    fn test_bounce_notification_invalid_status_rejected() {
        let json = serde_json::json!({
            "message_id": Uuid::new_v4(),
            "status": "invalid_status"
        });
        assert!(serde_json::from_value::<BounceNotification>(json).is_err());
    }

    #[test]
    fn test_bounce_notification_invalid_bounce_type_rejected() {
        let json = serde_json::json!({
            "message_id": Uuid::new_v4(),
            "status": "bounced",
            "bounce_type": "unknown"
        });
        assert!(serde_json::from_value::<BounceNotification>(json).is_err());
    }

    #[test]
    fn test_auto_disable_threshold_is_five() {
        assert_eq!(AUTO_DISABLE_THRESHOLD, 5);
    }
}
