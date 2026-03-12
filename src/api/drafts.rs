use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::auth::AuthOrg;
use super::error::ApiError;
use super::{clamp_pagination, get_inbox_for_org, AppState, PaginationParams};
use crate::models::{Draft, Message};

#[derive(Deserialize)]
pub struct CreateDraftRequest {
    pub to: Option<Vec<String>>,
    pub cc: Option<Vec<String>>,
    pub subject: Option<String>,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
    pub in_reply_to_message_id: Option<Uuid>,
}

#[derive(Deserialize)]
pub struct UpdateDraftRequest {
    pub to: Option<Vec<String>>,
    pub cc: Option<Vec<String>>,
    pub subject: Option<String>,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path(inbox_id): Path<Uuid>,
    Json(req): Json<CreateDraftRequest>,
) -> Result<(StatusCode, Json<Draft>), ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    if let Some(reply_id) = req.in_reply_to_message_id {
        let orig = crate::db::messages::get_by_id(&state.pool, reply_id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
            .ok_or(ApiError::BadRequest(
                "in_reply_to_message_id not found".into(),
            ))?;
        if orig.inbox_id != inbox_id {
            return Err(ApiError::BadRequest(
                "in_reply_to_message_id must belong to the same inbox".into(),
            ));
        }
    }

    let cd = crate::models::CreateDraft {
        inbox_id,
        to_addrs: serde_json::json!(req.to.unwrap_or_default()),
        cc_addrs: req.cc.map(|cc| serde_json::json!(cc)),
        subject: req.subject,
        text_body: req.text_body,
        html_body: req.html_body,
        in_reply_to_message_id: req.in_reply_to_message_id,
    };

    let draft = crate::db::drafts::create(&state.pool, &cd)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok((StatusCode::CREATED, Json(draft)))
}

pub async fn list(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path(inbox_id): Path<Uuid>,
    Query(params): Query<PaginationParams>,
) -> Result<Json<Vec<Draft>>, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let (limit, offset) = clamp_pagination(&params);
    let drafts = crate::db::drafts::list_by_inbox(&state.pool, inbox_id, limit, offset)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(drafts))
}

pub async fn get(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path((inbox_id, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Draft>, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let draft = crate::db::drafts::get_by_id(&state.pool, id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound)?;

    if draft.inbox_id != inbox_id {
        return Err(ApiError::NotFound);
    }

    Ok(Json(draft))
}

pub async fn update(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path((inbox_id, id)): Path<(Uuid, Uuid)>,
    Json(req): Json<UpdateDraftRequest>,
) -> Result<Json<Draft>, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let existing = crate::db::drafts::get_by_id(&state.pool, id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound)?;

    if existing.inbox_id != inbox_id {
        return Err(ApiError::NotFound);
    }

    let to_addrs = req
        .to
        .map(|t| serde_json::json!(t))
        .unwrap_or(existing.to_addrs);
    let cc_addrs = match req.cc {
        Some(cc) => Some(serde_json::json!(cc)),
        None => existing.cc_addrs,
    };
    let subject = req.subject.or(existing.subject);
    let text_body = req.text_body.or(existing.text_body);
    let html_body = req.html_body.or(existing.html_body);

    let updated = crate::db::drafts::update(
        &state.pool,
        id,
        &to_addrs,
        cc_addrs.as_ref(),
        subject.as_deref(),
        text_body.as_deref(),
        html_body.as_deref(),
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?
    .ok_or(ApiError::NotFound)?;

    Ok(Json(updated))
}

pub async fn delete(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path((inbox_id, id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let draft = crate::db::drafts::get_by_id(&state.pool, id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound)?;

    if draft.inbox_id != inbox_id {
        return Err(ApiError::NotFound);
    }

    crate::db::drafts::delete(&state.pool, id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn send_draft(
    State(state): State<AppState>,
    AuthOrg(org_id): AuthOrg,
    Path((inbox_id, id)): Path<(Uuid, Uuid)>,
) -> Result<(StatusCode, Json<Message>), ApiError> {
    let inbox = get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let draft = crate::db::drafts::get_by_id(&state.pool, id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .ok_or(ApiError::NotFound)?;

    if draft.inbox_id != inbox_id {
        return Err(ApiError::NotFound);
    }

    let to: Vec<String> = serde_json::from_value(draft.to_addrs.clone())
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    if to.is_empty() {
        return Err(ApiError::BadRequest(
            "at least one recipient required".into(),
        ));
    }

    if let Err(violations) = crate::mail::guard::scan(
        draft.subject.as_deref(),
        draft.text_body.as_deref(),
        draft.html_body.as_deref(),
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

    let cc: Vec<String> = draft
        .cc_addrs
        .as_ref()
        .map(|v| serde_json::from_value(v.clone()))
        .transpose()
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .unwrap_or_default();

    let message_id = format!("{}@postblox", Uuid::new_v4());
    let mime_message_id = format!("<{message_id}>");
    let raw_mime = crate::mail::builder::build_mime(
        &inbox.email,
        &to,
        &cc,
        draft.subject.as_deref().unwrap_or(""),
        draft.text_body.as_deref(),
        draft.html_body.as_deref(),
        &mime_message_id,
    );

    let in_reply_to = if let Some(reply_id) = draft.in_reply_to_message_id {
        let orig = crate::db::messages::get_by_id(&state.pool, reply_id)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        orig.and_then(|m| m.message_id_header)
    } else {
        None
    };

    let cm = crate::models::CreateMessage {
        inbox_id,
        thread_id: None,
        message_id_header: Some(message_id),
        in_reply_to: in_reply_to.clone(),
        references_header: in_reply_to,
        from_addr: inbox.email.clone(),
        to_addrs: serde_json::json!(&to),
        cc_addrs: if cc.is_empty() {
            None
        } else {
            Some(serde_json::json!(&cc))
        },
        subject: draft.subject,
        text_body: draft.text_body,
        html_body: draft.html_body,
        extracted_text: None,
        direction: "outbound".into(),
        raw_headers: None,
    };

    let msg = crate::db::messages::create(&state.pool, &cm)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    if let Some(ref stalwart) = state.stalwart {
        let to_refs: Vec<&str> = to.iter().map(|s| s.as_str()).collect();
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

    if let Err(e) = crate::db::drafts::delete(&state.pool, id).await {
        tracing::warn!("failed to delete draft {id} after send: {e}");
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
