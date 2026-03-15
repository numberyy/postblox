use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use super::auth::AuthOrg;
use super::error::ApiError;
use super::{
    check_send_allowed, get_inbox_for_org, get_message_for_inbox, spawn_approval_event, AppState,
    SendCheck,
};
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

pub struct SendParams {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: Option<String>,
    pub text_body: Option<String>,
    pub html_body: Option<String>,
}

pub async fn send_message_inner(
    state: &AppState,
    org_id: Uuid,
    inbox: &crate::models::Inbox,
    params: &SendParams,
    actor: &str,
) -> Result<(StatusCode, Message), ApiError> {
    let send_mode = check_send_allowed(
        state,
        inbox.id,
        &SendCheck {
            to: &params.to,
            subject: params.subject.as_deref(),
            text_body: params.text_body.as_deref(),
            html_body: params.html_body.as_deref(),
            from_addr: &inbox.email,
        },
    )
    .await?;

    let message_id = super::new_message_id();
    let cm = crate::models::CreateMessage {
        inbox_id: inbox.id,
        thread_id: None,
        message_id_header: Some(message_id.clone()),
        in_reply_to: None,
        references_header: None,
        from_addr: inbox.email.clone(),
        to_addrs: serde_json::json!(&params.to),
        cc_addrs: if params.cc.is_empty() {
            None
        } else {
            Some(serde_json::json!(&params.cc))
        },
        subject: params.subject.clone(),
        text_body: params.text_body.clone(),
        html_body: params.html_body.clone(),
        extracted_text: None,
        direction: crate::models::Direction::Outbound,
        raw_headers: None,
    };

    let msg = crate::db::messages::create(&state.pool, &cm)
        .await
        .map_err(ApiError::from_sqlx)?;

    if send_mode == SendMode::Approval {
        let approval = crate::db::approvals::create(
            &state.pool,
            &crate::models::CreateApproval {
                org_id,
                inbox_id: inbox.id,
                message_id: msg.id,
            },
        )
        .await
        .map_err(ApiError::from_sqlx)?;

        spawn_approval_event(state, org_id, inbox.id, msg.id, approval.id);
        return Ok((StatusCode::ACCEPTED, msg));
    }

    super::deliver::deliver_message(
        state,
        org_id,
        inbox,
        msg.id,
        &super::deliver::DeliveryParams {
            from: &inbox.email,
            to: &params.to,
            cc: &params.cc,
            subject: params.subject.as_deref().unwrap_or(""),
            text_body: params.text_body.as_deref(),
            html_body: params.html_body.as_deref(),
            message_id_header: &message_id,
            attachments: &[],
        },
    )
    .await?;

    let pool = state.pool.clone();
    let msg_id = msg.id;
    let actor = actor.to_string();
    let iid = inbox.id;
    tokio::spawn(async move {
        crate::events::audit(
            &pool,
            org_id,
            Some(iid),
            crate::models::AuditAction::MessageSent,
            &actor,
            serde_json::json!({"message_id": msg_id.to_string()}),
        )
        .await;
    });

    Ok((StatusCode::CREATED, msg))
}

pub async fn send(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path(inbox_id): Path<Uuid>,
    Json(req): Json<SendMessageRequest>,
) -> Result<(StatusCode, Json<Message>), ApiError> {
    let inbox = get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    if req.to.is_empty() {
        return Err(ApiError::BadRequest(
            "at least one recipient required".into(),
        ));
    }

    let (status, msg) = send_message_inner(
        &state,
        org_id,
        &inbox,
        &SendParams {
            to: req.to,
            cc: req.cc.unwrap_or_default(),
            subject: req.subject,
            text_body: req.text_body,
            html_body: req.html_body,
        },
        "api",
    )
    .await?;

    Ok((status, Json(msg)))
}

pub async fn list(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path(inbox_id): Path<Uuid>,
    Query(params): Query<ListMessagesParams>,
) -> Result<Json<Vec<Message>>, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let (limit, offset) = super::clamp_pagination_raw(params.limit, params.offset);

    let unslopify = params.unslopify.unwrap_or(false);

    let messages = if let Some(thread_id) = params.thread_id {
        let thread = crate::db::threads::get_by_id(&state.pool, thread_id)
            .await
            .map_err(ApiError::from_sqlx)?
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
    .map_err(ApiError::from_sqlx)?;

    Ok(Json(messages))
}

pub async fn get(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path((inbox_id, id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Message>, ApiError> {
    get_inbox_for_org(&state.pool, inbox_id, org_id).await?;
    let msg = get_message_for_inbox(&state.pool, id, inbox_id).await?;
    Ok(Json(msg))
}

struct UploadedFile {
    filename: String,
    content_type: String,
    data: Vec<u8>,
}

async fn cleanup_stored(storage_path: &std::path::Path, keys: &[String]) {
    for key in keys {
        if let Err(e) = crate::storage::delete_attachment(storage_path, key).await {
            tracing::warn!(storage_key = %key, "failed to clean up attachment after error: {e}");
        }
    }
}

pub async fn send_with_attachments(
    State(state): State<AppState>,
    AuthOrg { org_id, .. }: AuthOrg,
    Path(inbox_id): Path<Uuid>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<Message>), ApiError> {
    let inbox = get_inbox_for_org(&state.pool, inbox_id, org_id).await?;

    let mut metadata: Option<SendMessageRequest> = None;
    let mut files: Vec<UploadedFile> = Vec::new();
    let max_size = state.max_attachment_size_bytes as usize;
    const MAX_ATTACHMENTS: usize = 20;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "metadata" => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("failed to read metadata: {e}")))?;
                metadata =
                    Some(serde_json::from_slice(&bytes).map_err(|e| {
                        ApiError::BadRequest(format!("invalid metadata JSON: {e}"))
                    })?);
            }
            "file" => {
                if files.len() >= MAX_ATTACHMENTS {
                    return Err(ApiError::BadRequest(format!(
                        "too many attachments (max {MAX_ATTACHMENTS})"
                    )));
                }
                let filename = field.file_name().unwrap_or("attachment").to_string();
                let content_type = field
                    .content_type()
                    .unwrap_or("application/octet-stream")
                    .to_string();
                if let crate::core::content_filter::FilterResult::Block(reason) =
                    state.content_filter.check(&content_type)
                {
                    return Err(ApiError::BadRequest(reason));
                }
                let data = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::BadRequest(format!("failed to read file: {e}")))?;
                if data.len() > max_size {
                    return Err(ApiError::BadRequest(format!(
                        "attachment '{}' exceeds max size of {} bytes",
                        filename, max_size
                    )));
                }
                files.push(UploadedFile {
                    filename,
                    content_type,
                    data: data.into(),
                });
            }
            other => {
                return Err(ApiError::BadRequest(format!(
                    "unexpected multipart field '{other}'; expected 'metadata' or 'file'"
                )));
            }
        }
    }

    let req = metadata.ok_or_else(|| ApiError::BadRequest("missing 'metadata' field".into()))?;

    if req.to.is_empty() {
        return Err(ApiError::BadRequest(
            "at least one recipient required".into(),
        ));
    }

    let send_mode = check_send_allowed(
        &state,
        inbox_id,
        &SendCheck {
            to: &req.to,
            subject: req.subject.as_deref(),
            text_body: req.text_body.as_deref(),
            html_body: req.html_body.as_deref(),
            from_addr: &inbox.email,
        },
    )
    .await?;

    let message_id = super::new_message_id();

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
        direction: crate::models::Direction::Outbound,
        raw_headers: None,
    };

    let msg = crate::db::messages::create(&state.pool, &cm)
        .await
        .map_err(ApiError::from_sqlx)?;

    let mut stored_keys: Vec<String> = Vec::new();
    let mut mime_attachments: Vec<crate::mail::builder::MimeAttachment> = Vec::new();
    let msg_id_str = msg.id.to_string();
    let storage_path = &state.attachment_storage_path;
    let mut seen_names = std::collections::HashSet::new();

    for file in files {
        let filename = if !seen_names.insert(file.filename.clone()) {
            let (stem, ext) = file
                .filename
                .rsplit_once('.')
                .map(|(s, e)| (s, Some(e)))
                .unwrap_or((&file.filename, None));
            let mut n = 1u32;
            loop {
                let candidate = match ext {
                    Some(e) => format!("{stem}_{n}.{e}"),
                    None => format!("{stem}_{n}"),
                };
                if seen_names.insert(candidate.clone()) {
                    break candidate;
                }
                n += 1;
            }
        } else {
            file.filename.clone()
        };

        let storage_key = match crate::storage::store_attachment(
            storage_path,
            &msg_id_str,
            &filename,
            &file.data,
            max_size as u64,
        )
        .await
        {
            Ok(key) => key,
            Err(e) => {
                cleanup_stored(storage_path, &stored_keys).await;
                return Err(ApiError::Internal(format!(
                    "failed to store attachment: {e}"
                )));
            }
        };
        stored_keys.push(storage_key.clone());

        if let Err(e) = crate::db::attachments::create(
            &state.pool,
            &crate::models::CreateAttachment {
                message_id: msg.id,
                filename: filename.clone(),
                content_type: file.content_type.clone(),
                size_bytes: file.data.len() as i64,
                storage_key,
                disposition: crate::models::Disposition::Attachment,
                content_id: None,
            },
        )
        .await
        {
            cleanup_stored(storage_path, &stored_keys).await;
            return Err(ApiError::from_sqlx(e));
        }

        mime_attachments.push(crate::mail::builder::MimeAttachment {
            filename,
            content_type: file.content_type,
            data: file.data,
            content_id: None,
        });
    }

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
        .map_err(ApiError::from_sqlx)?;

        spawn_approval_event(&state, org_id, inbox_id, msg.id, approval.id);
        return Ok((StatusCode::ACCEPTED, Json(msg)));
    }

    let cc = req.cc.as_deref().unwrap_or(&[]);
    if let Err(e) = super::deliver::deliver_message(
        &state,
        org_id,
        &inbox,
        msg.id,
        &super::deliver::DeliveryParams {
            from: &inbox.email,
            to: &req.to,
            cc,
            subject: req.subject.as_deref().unwrap_or(""),
            text_body: req.text_body.as_deref(),
            html_body: req.html_body.as_deref(),
            message_id_header: &message_id,
            attachments: &mime_attachments,
        },
    )
    .await
    {
        tracing::warn!(
            message_id = %msg.id,
            attachment_count = stored_keys.len(),
            "delivery failed after attachments stored; files retained on disk"
        );
        return Err(e);
    }

    let pool = state.pool.clone();
    let msg_id = msg.id;
    tokio::spawn(async move {
        crate::events::audit(
            &pool,
            org_id,
            Some(inbox_id),
            crate::models::AuditAction::MessageSent,
            "api",
            serde_json::json!({"message_id": msg_id.to_string()}),
        )
        .await;
    });

    Ok((StatusCode::CREATED, Json(msg)))
}
