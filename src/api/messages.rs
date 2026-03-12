use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::auth::AuthOrg;
use super::error::ApiError;
use super::{get_inbox_for_org, AppState};
use crate::models::Message;

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

    // Store without angle brackets for consistent threading with inbound (parser strips them).
    let message_id = format!("{}@postblox", Uuid::new_v4());
    let mime_message_id = format!("<{message_id}>");
    let raw_mime = crate::mail::builder::build_mime(
        &inbox.email,
        &req.to,
        req.cc.as_deref().unwrap_or(&[]),
        req.subject.as_deref().unwrap_or(""),
        req.text_body.as_deref(),
        req.html_body.as_deref(),
        &mime_message_id,
    );

    // DB write first so the message is tracked even if delivery fails.
    let cm = crate::models::CreateMessage {
        inbox_id,
        thread_id: None,
        message_id_header: Some(message_id),
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

    if let Some(ref stalwart) = state.stalwart {
        let to_refs: Vec<&str> = req.to.iter().map(|s| s.as_str()).collect();
        if let Err(e) = stalwart
            .submit_message(&inbox.email, &to_refs, &raw_mime)
            .await
        {
            tracing::error!("stalwart submission failed: {e}");
            return Err(ApiError::Internal("email delivery failed".into()));
        }
    } else {
        tracing::warn!("stalwart not configured, skipping email delivery");
    }

    let pool = state.pool.clone();
    let webhook_client = state.webhook_client.clone();
    let msg_id = msg.id;
    tokio::spawn(async move {
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

    let messages = if let Some(thread_id) = params.thread_id {
        let thread = crate::db::threads::get_by_id(&state.pool, thread_id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
            .ok_or(ApiError::NotFound)?;
        if thread.inbox_id != inbox_id {
            return Err(ApiError::NotFound);
        }
        crate::db::messages::list_by_thread(&state.pool, thread_id).await
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
