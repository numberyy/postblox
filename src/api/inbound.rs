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

    let (threads_result, mid_map_result) = tokio::join!(
        crate::db::threads::list_by_inbox(&state.pool, inbox.id, 10_000, 0),
        crate::db::messages::message_id_headers_by_inbox(&state.pool, inbox.id, 10_000),
    );
    let threads = threads_result.map_err(|e| ApiError::Internal(e.to_string()))?;
    let mid_map = mid_map_result.map_err(|e| ApiError::Internal(e.to_string()))?;

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
            let thread = crate::db::threads::create_with_message(
                &state.pool,
                inbox.id,
                parsed.subject.as_deref(),
                Utc::now(),
            )
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
            thread.id
        }
    };

    let cm =
        crate::mail::parsed_to_create_message(&parsed, inbox.id, Some(thread_id), extracted_text);

    let msg = crate::db::messages::create(&state.pool, &cm)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let sender_rep =
        match crate::db::slop::get_sender_reputation(&state.pool, inbox.org_id, &parsed.from).await
        {
            Ok(rep) => rep,
            Err(e) => {
                tracing::warn!(message_id = %msg.id, "failed to fetch sender reputation: {e}");
                None
            }
        };
    let slop_ratio = sender_rep.map(|r| r.slop_ratio());
    let classifier_input = crate::core::slop::ClassifierInput {
        from_addr: &parsed.from,
        subject: parsed.subject.as_deref(),
        text_body: parsed.text_body.as_deref(),
        raw_headers: Some(&parsed.raw_headers),
        sender_slop_ratio: slop_ratio,
    };
    let slop_result = crate::core::slop::classify(&classifier_input);
    let signals_json = serde_json::json!(slop_result.signals);
    let slop_fields = crate::db::slop::SlopFields {
        score: slop_result.score,
        signals: &signals_json,
        category: slop_result.category.as_deref(),
        priority: &slop_result.priority,
        triage_status: slop_result.triage_action.as_str(),
        requires_action: slop_result.requires_action,
    };
    let (slop_update, rep_update) = tokio::join!(
        crate::db::slop::update_slop_fields(&state.pool, msg.id, &slop_fields),
        crate::db::slop::upsert_sender_reputation(
            &state.pool,
            inbox.org_id,
            &parsed.from,
            slop_result.is_slop()
        ),
    );
    if let Err(e) = slop_update {
        tracing::warn!(message_id = %msg.id, "failed to update slop fields: {e}");
    }
    if let Err(e) = rep_update {
        tracing::warn!(message_id = %msg.id, "failed to upsert sender reputation: {e}");
    }

    let pool = state.pool.clone();
    let webhook_client = state.webhook_client.clone();
    let hooks = state.hooks.clone();
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
            &hooks,
        )
        .await;
        crate::events::dispatch(
            &pool,
            org_id,
            crate::events::PostbloxEvent::MessageClassified {
                message_id: msg_id,
                inbox_id,
            },
            &webhook_client,
            &hooks,
        )
        .await;
    });

    if let Some(ref provider) = state.embedding_provider {
        let text = cm
            .extracted_text
            .clone()
            .or_else(|| cm.text_body.clone())
            .unwrap_or_default();
        if !text.is_empty() {
            let pool = state.pool.clone();
            let provider = provider.clone();
            let semaphore = state.embedding_semaphore.clone();
            let msg_id = msg.id;
            tokio::spawn(async move {
                let _permit = match semaphore.acquire().await {
                    Ok(p) => p,
                    Err(_) => return,
                };
                match provider.embed(&text).await {
                    Ok(embedding) => {
                        if let Err(e) =
                            crate::db::embeddings::store_embedding(&pool, msg_id, &embedding).await
                        {
                            tracing::warn!(message_id = %msg_id, "failed to store embedding: {e}");
                        }
                    }
                    Err(e) => {
                        tracing::warn!(message_id = %msg_id, "failed to generate embedding: {e}");
                    }
                }
            });
        }
    }

    Ok(StatusCode::OK)
}
