use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::auth::AuthOrg;
use super::error::ApiError;
use super::AppState;
use crate::models::SlopFeedback;

#[derive(Deserialize, Serialize)]
pub struct FeedbackRequest {
    pub message_id: Uuid,
    pub is_slop: bool,
}

pub async fn submit(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Json(req): Json<FeedbackRequest>,
) -> Result<(StatusCode, Json<SlopFeedback>), ApiError> {
    let msg = crate::db::messages::get_by_id(&state.pool, req.message_id)
        .await
        .map_err(ApiError::from_sqlx)?
        .ok_or(ApiError::NotFound)?;

    super::get_inbox_for_org(&state.pool, msg.inbox_id, org_id).await?;

    let (feedback_result, rep_result) = tokio::join!(
        crate::db::slop_feedback::create(&state.pool, org_id, req.message_id, req.is_slop),
        crate::db::slop::upsert_sender_reputation(&state.pool, org_id, &msg.from_addr, req.is_slop),
    );
    let feedback = feedback_result.map_err(ApiError::from_sqlx)?;
    rep_result.map_err(ApiError::from_sqlx)?;

    Ok((StatusCode::CREATED, Json(feedback)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feedback_request_deserialize() {
        let json = r#"{"message_id": "00000000-0000-0000-0000-000000000001", "is_slop": true}"#;
        let req: FeedbackRequest = serde_json::from_str(json).unwrap();
        assert_eq!(
            req.message_id,
            "00000000-0000-0000-0000-000000000001"
                .parse::<Uuid>()
                .unwrap()
        );
        assert!(req.is_slop);
    }

    #[test]
    fn test_feedback_request_deserialize_not_slop() {
        let json = r#"{"message_id": "00000000-0000-0000-0000-000000000002", "is_slop": false}"#;
        let req: FeedbackRequest = serde_json::from_str(json).unwrap();
        assert!(!req.is_slop);
    }

    #[test]
    fn test_feedback_request_missing_message_id_fails() {
        let json = r#"{"is_slop": true}"#;
        let result = serde_json::from_str::<FeedbackRequest>(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_feedback_request_missing_is_slop_fails() {
        let json = r#"{"message_id": "00000000-0000-0000-0000-000000000001"}"#;
        let result = serde_json::from_str::<FeedbackRequest>(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_feedback_request_invalid_uuid_fails() {
        let json = r#"{"message_id": "not-a-uuid", "is_slop": true}"#;
        let result = serde_json::from_str::<FeedbackRequest>(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_feedback_request_roundtrip() {
        let req = FeedbackRequest {
            message_id: Uuid::new_v4(),
            is_slop: true,
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: FeedbackRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req.message_id, back.message_id);
        assert_eq!(req.is_slop, back.is_slop);
    }
}
