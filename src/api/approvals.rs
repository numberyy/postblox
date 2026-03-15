use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::auth::AuthOrg;
use super::error::ApiError;
use super::AppState;
use crate::models::{Approval, ApprovalStatus, ApprovalWithDetails};

pub async fn record_trust_and_maybe_upgrade(
    state: &AppState,
    org_id: Uuid,
    inbox_id: Uuid,
    approved: bool,
) {
    if let Err(e) = crate::db::trust::record_send_outcome(&state.pool, inbox_id, approved).await {
        tracing::error!("failed to record trust outcome: {e}");
    }
    if !approved {
        return;
    }
    match crate::db::trust::check_and_upgrade(
        &state.pool,
        inbox_id,
        state.trust_auto_upgrade_threshold,
    )
    .await
    {
        Ok(Some(score)) => {
            crate::events::audit(
                &state.pool,
                org_id,
                Some(inbox_id),
                crate::models::AuditAction::PermissionChanged,
                "system:trust_auto_upgrade",
                serde_json::json!({
                    "new_mode": "auto_approve",
                    "approved_count": score.approved_count,
                    "threshold": state.trust_auto_upgrade_threshold,
                }),
            )
            .await;
            crate::events::dispatch(
                &state.pool,
                org_id,
                crate::events::PostbloxEvent::TrustChanged {
                    inbox_id,
                    new_mode: crate::models::SendMode::AutoApprove,
                    approved_count: score.approved_count,
                },
                &state.webhook_client,
                &state.hooks,
                &state.ws_hub,
            )
            .await;
        }
        Ok(None) => {}
        Err(e) => tracing::error!("failed to check trust upgrade: {e}"),
    }
}

pub async fn execute_approval(
    state: &AppState,
    org_id: Uuid,
    approval: &Approval,
    decided_by: &str,
) -> Result<(), ApiError> {
    let (msg_result, inbox_result) = tokio::join!(
        crate::db::messages::get_by_id(&state.pool, approval.message_id),
        crate::db::inboxes::get_by_id(&state.pool, approval.inbox_id),
    );
    let msg = msg_result
        .map_err(ApiError::from_sqlx)?
        .ok_or(ApiError::Internal("approved message not found".into()))?;
    let inbox = inbox_result
        .map_err(ApiError::from_sqlx)?
        .ok_or(ApiError::Internal("inbox not found".into()))?;

    let (to, cc) = super::deliver::extract_addrs(&msg);
    super::deliver::deliver_message(
        state,
        org_id,
        &inbox,
        approval.message_id,
        &super::deliver::DeliveryParams {
            from: &inbox.email,
            to: &to,
            cc: &cc,
            subject: msg.subject.as_deref().unwrap_or(""),
            text_body: msg.text_body.as_deref(),
            html_body: msg.html_body.as_deref(),
            message_id_header: &msg
                .message_id_header
                .clone()
                .unwrap_or_else(super::new_message_id),
            attachments: &[],
        },
    )
    .await?;

    crate::events::audit(
        &state.pool,
        org_id,
        Some(approval.inbox_id),
        crate::models::AuditAction::MessageApproved,
        decided_by,
        serde_json::json!({"message_id": approval.message_id.to_string(), "approval_id": approval.id.to_string()}),
    )
    .await;

    record_trust_and_maybe_upgrade(state, org_id, approval.inbox_id, true).await;

    Ok(())
}

#[derive(Deserialize)]
pub struct ApprovalListParams {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub status: Option<String>,
}

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
    AuthOrg { org_id, .. }: AuthOrg,
    Query(params): Query<ApprovalListParams>,
) -> Result<Json<Vec<ApprovalWithDetails>>, ApiError> {
    let (limit, offset) = super::clamp_pagination_raw(params.limit, params.offset);

    let approvals = crate::db::approvals::list_with_details(
        &state.pool,
        org_id,
        params.status.as_deref(),
        offset,
        limit,
    )
    .await
    .map_err(ApiError::from_sqlx)?;

    Ok(Json(approvals))
}

pub async fn get(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path(id): Path<Uuid>,
) -> Result<Json<Approval>, ApiError> {
    crate::db::approvals::get(&state.pool, org_id, id)
        .await
        .map_err(ApiError::from_sqlx)?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

pub async fn approve(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path(id): Path<Uuid>,
    Json(req): Json<DecisionRequest>,
) -> Result<Json<Approval>, ApiError> {
    let approval = crate::db::approvals::approve(&state.pool, org_id, id, &req.decided_by)
        .await
        .map_err(ApiError::from_sqlx)?
        .ok_or(ApiError::NotFound)?;

    execute_approval(&state, org_id, &approval, &req.decided_by).await?;

    Ok(Json(approval))
}

pub async fn reject(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path(id): Path<Uuid>,
    Json(req): Json<DecisionRequest>,
) -> Result<Json<Approval>, ApiError> {
    let approval = crate::db::approvals::reject(&state.pool, org_id, id, &req.decided_by)
        .await
        .map_err(ApiError::from_sqlx)?
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

        if let Err(e) = crate::db::trust::record_send_outcome(&pool, inbox_id, false).await {
            tracing::error!("failed to record trust outcome: {e}");
        }
    });

    Ok(Json(approval))
}

pub async fn batch(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
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
    .map_err(ApiError::from_sqlx)?;

    let state_clone = state.clone();
    let status = req.status;
    let decided_by = req.decided_by.clone();
    let decided_clone = decided.clone();
    tokio::spawn(async move {
        for d in &decided_clone {
            if status == ApprovalStatus::Approved {
                if let Err(e) = execute_approval(&state_clone, org_id, d, &decided_by).await {
                    tracing::error!(
                        "batch approve: delivery failed for message {}: {e:?}",
                        d.message_id
                    );
                }
            } else {
                crate::events::audit(
                    &state_clone.pool,
                    org_id,
                    Some(d.inbox_id),
                    crate::models::AuditAction::MessageRejected,
                    &decided_by,
                    serde_json::json!({"message_id": d.message_id.to_string(), "approval_id": d.id.to_string(), "batch": true}),
                )
                .await;
                record_trust_and_maybe_upgrade(&state_clone, org_id, d.inbox_id, false).await;
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
