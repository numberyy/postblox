use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::auth::AuthOrg;
use super::error::ApiError;
use super::{clamp_pagination, AppState, PaginationParams};
use crate::models::{Approval, ApprovalStatus};

#[derive(Deserialize)]
pub struct DecisionRequest {
    pub decided_by: String,
}

#[derive(Deserialize)]
pub struct BatchDecisionRequest {
    pub ids: Vec<Uuid>,
    pub status: ApprovalStatus,
    pub decided_by: String,
}

pub async fn list(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Query(params): Query<PaginationParams>,
) -> Result<Json<Vec<Approval>>, ApiError> {
    let (limit, offset) = clamp_pagination(&params);

    let approvals = crate::db::approvals::list_pending(&state.pool, org_id, offset, limit)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(approvals))
}

pub async fn get(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path(id): Path<Uuid>,
) -> Result<Json<Approval>, ApiError> {
    crate::db::approvals::get(&state.pool, org_id, id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

pub async fn approve(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path(id): Path<Uuid>,
    Json(req): Json<DecisionRequest>,
) -> Result<Json<Approval>, ApiError> {
    let approval = crate::db::approvals::approve(&state.pool, org_id, id, &req.decided_by)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound)?;

    let (msg_result, inbox_result) = tokio::join!(
        crate::db::messages::get_by_id(&state.pool, approval.message_id),
        crate::db::inboxes::get_by_id(&state.pool, approval.inbox_id),
    );
    let msg = msg_result
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::Internal("approved message not found".into()))?;
    let inbox = inbox_result
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::Internal("inbox not found".into()))?;

    let to_addrs: Vec<String> = msg
        .to_addrs
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let cc_addrs: Vec<String> = msg
        .cc_addrs
        .as_ref()
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let message_id_header = msg
        .message_id_header
        .as_deref()
        .unwrap_or("unknown@postblox");
    let mime_message_id = if message_id_header.starts_with('<') {
        message_id_header.to_string()
    } else {
        format!("<{message_id_header}>")
    };

    let raw_mime = crate::mail::builder::build_mime(
        &inbox.email,
        &to_addrs,
        &cc_addrs,
        msg.subject.as_deref().unwrap_or(""),
        msg.text_body.as_deref(),
        msg.html_body.as_deref(),
        &mime_message_id,
    );

    if let Some(ref stalwart) = state.stalwart {
        let to_refs: Vec<&str> = to_addrs.iter().map(|s| s.as_str()).collect();
        if let Err(e) = stalwart
            .submit_message(&inbox.email, &to_refs, &raw_mime)
            .await
        {
            tracing::error!("stalwart submission failed for approved message: {e}");
            return Err(ApiError::Internal("email delivery failed".into()));
        }
    } else {
        tracing::warn!("stalwart not configured, skipping delivery for approved message");
    }

    let pool = state.pool.clone();
    let webhook_client = state.webhook_client.clone();
    let msg_id = approval.message_id;
    let inbox_id = approval.inbox_id;
    let decided_by = req.decided_by.clone();
    tokio::spawn(async move {
        crate::events::audit(
            &pool,
            org_id,
            Some(inbox_id),
            crate::models::AuditAction::MessageApproved,
            &decided_by,
            serde_json::json!({"message_id": msg_id.to_string(), "approval_id": id.to_string()}),
        )
        .await;
        crate::events::dispatch(
            &pool,
            org_id,
            crate::events::PostbloxEvent::MessageSent {
                message_id: msg_id,
                inbox_id,
            },
            &webhook_client,
        )
        .await;
    });

    Ok(Json(approval))
}

pub async fn reject(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path(id): Path<Uuid>,
    Json(req): Json<DecisionRequest>,
) -> Result<Json<Approval>, ApiError> {
    let approval = crate::db::approvals::reject(&state.pool, org_id, id, &req.decided_by)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound)?;

    let pool = state.pool.clone();
    let inbox_id = approval.inbox_id;
    let msg_id = approval.message_id;
    let decided_by = req.decided_by.clone();
    tokio::spawn(async move {
        crate::events::audit(
            &pool,
            org_id,
            Some(inbox_id),
            crate::models::AuditAction::MessageRejected,
            &decided_by,
            serde_json::json!({"message_id": msg_id.to_string(), "approval_id": id.to_string()}),
        )
        .await;
    });

    Ok(Json(approval))
}

pub async fn batch(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Json(req): Json<BatchDecisionRequest>,
) -> Result<(StatusCode, Json<Vec<Approval>>), ApiError> {
    if req.status == ApprovalStatus::Pending {
        return Err(ApiError::BadRequest(
            "status must be 'approved' or 'rejected'".into(),
        ));
    }
    if req.ids.is_empty() {
        return Err(ApiError::BadRequest("ids must not be empty".into()));
    }

    let decided = crate::db::approvals::batch_decide(
        &state.pool,
        org_id,
        &req.ids,
        req.status,
        &req.decided_by,
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    let pool = state.pool.clone();
    let stalwart = state.stalwart.clone();
    let webhook_client = state.webhook_client.clone();
    let status = req.status;
    let decided_by = req.decided_by.clone();
    let decided_clone = decided.clone();
    tokio::spawn(async move {
        let action = if status == ApprovalStatus::Approved {
            crate::models::AuditAction::MessageApproved
        } else {
            crate::models::AuditAction::MessageRejected
        };
        for d in &decided_clone {
            crate::events::audit(
                &pool,
                org_id,
                Some(d.inbox_id),
                action,
                &decided_by,
                serde_json::json!({"message_id": d.message_id.to_string(), "approval_id": d.id.to_string(), "batch": true}),
            )
            .await;

            if status == ApprovalStatus::Approved {
                let (msg_result, inbox_result) = tokio::join!(
                    crate::db::messages::get_by_id(&pool, d.message_id),
                    crate::db::inboxes::get_by_id(&pool, d.inbox_id),
                );
                let msg = match msg_result {
                    Ok(Some(m)) => m,
                    _ => {
                        tracing::error!("batch approve: message {} not found", d.message_id);
                        continue;
                    }
                };
                let inbox = match inbox_result {
                    Ok(Some(i)) => i,
                    _ => {
                        tracing::error!("batch approve: inbox {} not found", d.inbox_id);
                        continue;
                    }
                };

                let to_addrs: Vec<String> = msg
                    .to_addrs
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let cc_addrs: Vec<String> = msg
                    .cc_addrs
                    .as_ref()
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                let message_id_header = msg
                    .message_id_header
                    .as_deref()
                    .unwrap_or("unknown@postblox");
                let mime_message_id = if message_id_header.starts_with('<') {
                    message_id_header.to_string()
                } else {
                    format!("<{message_id_header}>")
                };

                let raw_mime = crate::mail::builder::build_mime(
                    &inbox.email,
                    &to_addrs,
                    &cc_addrs,
                    msg.subject.as_deref().unwrap_or(""),
                    msg.text_body.as_deref(),
                    msg.html_body.as_deref(),
                    &mime_message_id,
                );

                if let Some(ref stalwart) = stalwart {
                    let to_refs: Vec<&str> = to_addrs.iter().map(|s| s.as_str()).collect();
                    if let Err(e) = stalwart
                        .submit_message(&inbox.email, &to_refs, &raw_mime)
                        .await
                    {
                        tracing::error!(
                            "batch approve: stalwart delivery failed for message {}: {e}",
                            d.message_id
                        );
                    }
                }

                crate::events::dispatch(
                    &pool,
                    org_id,
                    crate::events::PostbloxEvent::MessageSent {
                        message_id: d.message_id,
                        inbox_id: d.inbox_id,
                    },
                    &webhook_client,
                )
                .await;
            }
        }
    });

    Ok((StatusCode::OK, Json(decided)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decision_request_deserialize() {
        let json = r#"{"decided_by": "admin@example.com"}"#;
        let req: DecisionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.decided_by, "admin@example.com");
    }

    #[test]
    fn test_decision_request_missing_field_fails() {
        let json = r#"{}"#;
        let result = serde_json::from_str::<DecisionRequest>(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_batch_decision_request_deserialize() {
        let json = serde_json::json!({
            "ids": ["00000000-0000-0000-0000-000000000001", "00000000-0000-0000-0000-000000000002"],
            "status": "approved",
            "decided_by": "admin"
        });
        let req: BatchDecisionRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.ids.len(), 2);
        assert_eq!(req.status, ApprovalStatus::Approved);
        assert_eq!(req.decided_by, "admin");
    }

    #[test]
    fn test_batch_decision_request_empty_ids() {
        let json = serde_json::json!({
            "ids": [],
            "status": "rejected",
            "decided_by": "admin"
        });
        let req: BatchDecisionRequest = serde_json::from_value(json).unwrap();
        assert!(req.ids.is_empty());
    }

    #[test]
    fn test_batch_decision_request_invalid_status_fails() {
        let json = serde_json::json!({
            "ids": ["00000000-0000-0000-0000-000000000001"],
            "status": "invalid",
            "decided_by": "admin"
        });
        let result = serde_json::from_value::<BatchDecisionRequest>(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_batch_decision_request_missing_status_fails() {
        let json = serde_json::json!({
            "ids": ["00000000-0000-0000-0000-000000000001"],
            "decided_by": "admin"
        });
        let result = serde_json::from_value::<BatchDecisionRequest>(json);
        assert!(result.is_err());
    }
}
