use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use chrono::Utc;

use super::error::ApiError;
use super::AppState;

pub async fn receive_inbound(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
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

    let parsed = match crate::mail::parser::parse(&body) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("failed to parse inbound email: {e}");
            return Ok(StatusCode::OK);
        }
    };

    let mut inbox = None;
    for to_addr in &parsed.to {
        if let Some(found) = crate::db::inboxes::get_by_email(&state.pool, to_addr)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
        {
            inbox = Some(found);
            break;
        }
    }

    let inbox = match inbox {
        Some(i) => i,
        None => {
            tracing::warn!(to = ?parsed.to, "inbound email for unknown recipient");
            return Ok(StatusCode::OK);
        }
    };

    // Dedup: skip if we already stored this message.
    if let Some(ref mid) = parsed.message_id {
        if crate::db::messages::find_by_message_id_header(&state.pool, inbox.id, mid)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
            .is_some()
        {
            tracing::debug!(message_id = %mid, "duplicate inbound email, skipping");
            return Ok(StatusCode::OK);
        }
    }

    let extracted_text = parsed
        .text_body
        .as_ref()
        .map(|t| crate::mail::reply_extract::extract_reply(t));

    let threads = crate::db::threads::list_by_inbox(&state.pool, inbox.id, 100, 0)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Single query for all message_id_headers, grouped by thread — avoids N+1.
    let mid_map = crate::db::messages::message_id_headers_by_inbox(&state.pool, inbox.id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let thread_refs: Vec<_> = threads
        .iter()
        .map(|thread| crate::mail::ThreadRef {
            thread_id: thread.id,
            message_ids: mid_map.get(&thread.id).cloned().unwrap_or_default(),
            subject: thread.subject.clone().unwrap_or_default(),
            last_message_at: thread.last_message_at.unwrap_or(thread.created_at),
        })
        .collect();

    let thread_match = crate::mail::threading::assign_thread(&parsed, &thread_refs);

    let thread_id = match thread_match {
        crate::mail::ThreadMatch::Existing(id) => {
            crate::db::threads::increment_message_count(&state.pool, id, Utc::now())
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?;
            id
        }
        crate::mail::ThreadMatch::New => {
            let thread =
                crate::db::threads::create(&state.pool, inbox.id, parsed.subject.as_deref())
                    .await
                    .map_err(|e| ApiError::Internal(e.to_string()))?;
            crate::db::threads::increment_message_count(&state.pool, thread.id, Utc::now())
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?;
            thread.id
        }
    };

    let cm = crate::models::CreateMessage {
        inbox_id: inbox.id,
        thread_id: Some(thread_id),
        message_id_header: parsed.message_id.clone(),
        in_reply_to: parsed.in_reply_to.clone(),
        references_header: if parsed.references.is_empty() {
            None
        } else {
            Some(parsed.references.join(" "))
        },
        from_addr: parsed.from.clone(),
        to_addrs: serde_json::json!(parsed.to),
        cc_addrs: if parsed.cc.is_empty() {
            None
        } else {
            Some(serde_json::json!(parsed.cc))
        },
        subject: parsed.subject.clone(),
        text_body: parsed.text_body.clone(),
        html_body: parsed.html_body.clone(),
        extracted_text,
        direction: "inbound".into(),
        raw_headers: Some(parsed.raw_headers.clone()),
    };

    let msg = crate::db::messages::create(&state.pool, &cm)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let pool = state.pool.clone();
    let webhook_client = state.webhook_client.clone();
    let org_id = inbox.org_id;
    let inbox_id = inbox.id;
    let msg_id = msg.id;
    tokio::spawn(async move {
        crate::events::dispatch(
            &pool,
            org_id,
            crate::events::PostbloxEvent::MessageReceived {
                message_id: msg_id,
                inbox_id,
            },
            &webhook_client,
        )
        .await;
    });

    Ok(StatusCode::OK)
}
