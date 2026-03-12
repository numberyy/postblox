use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::auth::AuthOrg;
use super::error::ApiError;
use super::{get_inbox_for_org, AppState};
use crate::core::slop;
use crate::models::{Message, SendMode};

#[derive(Deserialize)]
pub struct SendMessageRequest {
    pub to: Vec<String>,
    pub cc: Option<Vec<String>>,
    pub subject: Option<String>,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
}

#[derive(Deserialize)]
pub struct ListMessagesParams {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub thread_id: Option<Uuid>,
    pub unslopify: Option<bool>,
}

pub async fn send(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path(inbox_id): Path<Uuid>,
    Json(req): Json<SendMessageRequest>,
) -> Result<(StatusCode, Json<Message>), ApiError> {
    let inbox = get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    if req.to.is_empty() {
        return Err(ApiError::BadRequest(
            "at least one recipient required".into(),
        ));
    }

    if let Err(violations) = crate::mail::guard::scan(
        req.subject.as_deref(),
        req.text_body.as_deref(),
        req.html_body.as_deref(),
        &state.guard_patterns,
    ) {
        let details: Vec<String> = violations
            .iter()
            .map(|v| format!("{} in {}", v.pattern_name, v.field))
            .collect();
        return Err(ApiError::BadRequest(format!(
            "message blocked: detected {}",
            details.join(", ")
        )));
    }

    let permission = match crate::db::permissions::get_by_inbox(&state.pool, inbox_id).await {
        Ok(perm) => perm,
        Err(e) => return Err(ApiError::Internal(e.to_string())),
    };
    let send_mode = permission.as_ref().map(|p| p.mode()).unwrap_or_default();

    match send_mode {
        SendMode::Shadow => {
            return Err(ApiError::Forbidden(
                "inbox is in shadow mode, sending disabled".into(),
            ));
        }
        SendMode::Approval => {}
        SendMode::AutoApprove => {
            if let Some(ref perm) = permission {
                let slop_score = {
                    let input = slop::ClassifierInput {
                        from_addr: &inbox.email,
                        subject: req.subject.as_deref(),
                        text_body: req.text_body.as_deref(),
                        raw_headers: None,
                        sender_slop_ratio: None,
                    };
                    slop::classify(&input).score as f64
                };
                if let crate::core::rules::RuleVerdict::Block { reason, .. } =
                    perm.rules().evaluate(
                        &req.to,
                        req.subject.as_deref().unwrap_or(""),
                        req.text_body.as_deref().unwrap_or(""),
                        Some(slop_score),
                    )
                {
                    return Err(ApiError::Forbidden(format!("rule check failed: {reason}")));
                }
            }
        }
        SendMode::Autonomous => {}
    }

    // Store without angle brackets for consistent threading with inbound (parser strips them).
    let message_id = format!("{}@postblox", Uuid::new_v4());

    // DB write first so the message is tracked even if delivery fails.
    let cm = crate::models::CreateMessage {
        inbox_id,
        thread_id: None,
        message_id_header: Some(message_id.clone()),
        in_reply_to: None,
        references_header: None,
        from_addr: inbox.email.clone(),
        to_addrs: serde_json::json!(&req.to),
        cc_addrs: req.cc.as_ref().map(|cc| serde_json::json!(cc)),
        subject: req.subject.clone(),
        text_body: req.text_body.clone(),
        html_body: req.html_body.clone(),
        extracted_text: None,
        direction: "outbound".into(),
        raw_headers: None,
    };

    let msg = crate::db::messages::create(&state.pool, &cm)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    if send_mode == SendMode::Approval {
        let approval = crate::db::approvals::create(
            &state.pool,
            &crate::models::CreateApproval {
                org_id,
                inbox_id,
                message_id: msg.id,
            },
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

        let pool = state.pool.clone();
        let webhook_client = state.webhook_client.clone();
        let msg_id = msg.id;
        tokio::spawn(async move {
            crate::events::dispatch(
                &pool,
                org_id,
                crate::events::PostbloxEvent::ApprovalRequested {
                    message_id: msg_id,
                    inbox_id,
                    approval_id: approval.id,
                },
                &webhook_client,
            )
            .await;
        });

        return Ok((StatusCode::ACCEPTED, Json(msg)));
    }

    let cc = req.cc.as_deref().unwrap_or(&[]);
    super::deliver::deliver_message(
        &state,
        org_id,
        inbox_id,
        msg.id,
        &super::deliver::DeliveryParams {
            from: &inbox.email,
            to: &req.to,
            cc,
            subject: req.subject.as_deref().unwrap_or(""),
            text_body: req.text_body.as_deref(),
            html_body: req.html_body.as_deref(),
            message_id_header: &message_id,
        },
    )
    .await?;

    Ok((StatusCode::CREATED, Json(msg)))
}

pub async fn list(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path(inbox_id): Path<Uuid>,
    Query(params): Query<ListMessagesParams>,
) -> Result<Json<Vec<Message>>, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let limit = params.limit.unwrap_or(50).clamp(1, 100);
    let offset = params.offset.unwrap_or(0).max(0);

    let unslopify = params.unslopify.unwrap_or(false);

    let messages = if let Some(thread_id) = params.thread_id {
        let thread = crate::db::threads::get_by_id(&state.pool, thread_id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
            .ok_or(ApiError::NotFound)?;
        if thread.inbox_id != inbox_id {
            return Err(ApiError::NotFound);
        }
        crate::db::messages::list_by_thread(&state.pool, thread_id).await
    } else if unslopify {
        crate::db::messages::list_by_inbox_unslopified(&state.pool, inbox_id, limit, offset).await
    } else {
        crate::db::messages::list_by_inbox(&state.pool, inbox_id, limit, offset).await
    }
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(messages))
}

pub async fn get(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path((inbox_id, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Message>, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let msg = crate::db::messages::get_by_id(&state.pool, id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound)?;

    if msg.inbox_id != inbox_id {
        return Err(ApiError::NotFound);
    }

    Ok(Json(msg))
}
