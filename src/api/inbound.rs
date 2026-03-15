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

    process_inbound_raw(&state, &body).await
}

pub async fn process_inbound_raw(state: &AppState, body: &[u8]) -> Result<StatusCode, ApiError> {
    let mut parsed = match crate::mail::parser::parse(body) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("failed to parse inbound email: {e}");
            return Ok(StatusCode::UNPROCESSABLE_ENTITY);
        }
    };

    let mut email_refs: Vec<&str> = parsed.to.iter().map(String::as_str).collect();
    email_refs.extend(parsed.cc.iter().map(String::as_str));
    let inbox = crate::db::inboxes::get_first_by_emails(&state.pool, &email_refs)
        .await
        .map_err(ApiError::from_sqlx)?;

    let inbox = match inbox {
        Some(i) => i,
        None => {
            tracing::warn!(to = ?parsed.to, "inbound email for unknown recipient");
            return Ok(StatusCode::NOT_FOUND);
        }
    };

    if parsed.message_id.is_none() {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(parsed.from.as_bytes());
        hasher.update(parsed.subject.as_deref().unwrap_or("").as_bytes());
        hasher.update(parsed.text_body.as_deref().unwrap_or("").as_bytes());
        hasher.update(chrono::Utc::now().timestamp_micros().to_le_bytes());
        parsed.message_id = Some(format!("synth-{:x}@postblox", hasher.finalize()));
    }

    let mid = parsed.message_id.as_deref().unwrap();
    if crate::db::messages::exists_by_message_id_header(&state.pool, inbox.id, mid)
        .await
        .map_err(ApiError::from_sqlx)?
    {
        tracing::debug!(message_id = %mid, "duplicate inbound email, skipping");
        return Ok(StatusCode::OK);
    }

    let extracted_text = parsed
        .text_body
        .as_ref()
        .map(|t| crate::mail::reply_extract::extract_reply(t));

    // Targeted threading: first try In-Reply-To/References via indexed lookup,
    // then fall back to subject matching with only recent threads.
    let mut ref_ids: Vec<&str> = Vec::new();
    if let Some(ref reply_to) = parsed.in_reply_to {
        ref_ids.push(reply_to);
    }
    ref_ids.extend(parsed.references.iter().map(String::as_str));

    let thread_match = if !ref_ids.is_empty() {
        match crate::db::threads::find_by_message_ids(&state.pool, inbox.id, &ref_ids)
            .await
            .map_err(ApiError::from_sqlx)?
        {
            Some(t) => crate::mail::ThreadMatch::Existing(t.id),
            None => {
                // References didn't match — try subject-based with recent threads
                subject_based_thread_match(state, inbox.id, &parsed).await?
            }
        }
    } else {
        subject_based_thread_match(state, inbox.id, &parsed).await?
    };

    let thread_id = match thread_match {
        crate::mail::ThreadMatch::Existing(id) => {
            crate::db::threads::increment_message_count(&state.pool, id, Utc::now())
                .await
                .map_err(ApiError::from_sqlx)?;
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
            .map_err(ApiError::from_sqlx)?;
            thread.id
        }
    };

    let cm =
        crate::mail::parsed_to_create_message(&parsed, inbox.id, Some(thread_id), extracted_text);

    let msg = crate::db::messages::create(&state.pool, &cm)
        .await
        .map_err(ApiError::from_sqlx)?;

    let mut attachment_count: usize = 0;
    let storage_path = &state.attachment_storage_path;
    let max_size = state.max_attachment_size_bytes as u64;
    let msg_id_str = msg.id.to_string();
    for att in &parsed.attachments {
        match crate::storage::store_attachment(
            storage_path,
            &msg_id_str,
            &att.filename,
            &att.data,
            max_size,
        )
        .await
        {
            Ok(storage_key) => {
                let create_att = crate::models::CreateAttachment {
                    message_id: msg.id,
                    filename: att.filename.clone(),
                    content_type: att.content_type.clone(),
                    size_bytes: att.data.len() as i64,
                    storage_key: storage_key.clone(),
                    disposition: att.disposition.clone(),
                };
                if let Err(e) = crate::db::attachments::create(&state.pool, &create_att).await {
                    tracing::error!(message_id = %msg.id, filename = %att.filename, "failed to store attachment metadata: {e}");
                    if let Err(cleanup) =
                        crate::storage::delete_attachment(storage_path, &storage_key).await
                    {
                        tracing::error!(message_id = %msg.id, storage_key = %storage_key, "failed to clean up orphaned attachment: {cleanup}");
                    }
                } else {
                    attachment_count += 1;
                }
            }
            Err(e) => {
                tracing::warn!(message_id = %msg.id, filename = %att.filename, "failed to store attachment file: {e}");
            }
        }
    }
    if attachment_count > 0 {
        tracing::info!(message_id = %msg.id, count = attachment_count, "stored inbound attachments");
    }

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
        category: slop_result.category,
        priority: slop_result.priority,
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
        tracing::error!(message_id = %msg.id, "failed to update slop fields: {e}");
    }
    if let Err(e) = rep_update {
        tracing::error!(message_id = %msg.id, "failed to upsert sender reputation: {e}");
    }

    let pool = state.pool.clone();
    let webhook_client = state.webhook_client.clone();
    let hooks = state.hooks.clone();
    let ws_hub = state.ws_hub.clone();
    let org_id = inbox.org_id;
    let inbox_id = inbox.id;
    let msg_id = msg.id;
    tokio::spawn(async move {
        tokio::join!(
            crate::events::dispatch(
                &pool,
                org_id,
                crate::events::PostbloxEvent::MessageReceived {
                    message_id: msg_id,
                    inbox_id,
                },
                &webhook_client,
                &hooks,
                &ws_hub,
            ),
            crate::events::dispatch(
                &pool,
                org_id,
                crate::events::PostbloxEvent::MessageClassified {
                    message_id: msg_id,
                    inbox_id,
                },
                &webhook_client,
                &hooks,
                &ws_hub,
            ),
        );
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
                    Err(_) => {
                        tracing::debug!("embedding semaphore closed, skipping");
                        return;
                    }
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

async fn subject_based_thread_match(
    state: &AppState,
    inbox_id: uuid::Uuid,
    parsed: &crate::mail::parser::ParsedEmail,
) -> Result<crate::mail::ThreadMatch, ApiError> {
    let cutoff = Utc::now() - chrono::Duration::days(7);
    let recent_threads =
        crate::db::threads::list_recent_by_inbox(&state.pool, inbox_id, cutoff, 200)
            .await
            .map_err(ApiError::from_sqlx)?;

    let thread_refs: Vec<_> = recent_threads
        .iter()
        .map(|t| crate::mail::ThreadRef {
            thread_id: t.id,
            message_ids: vec![],
            subject: t.subject.clone().unwrap_or_default(),
            last_message_at: t.last_message_at.unwrap_or(t.created_at),
        })
        .collect();

    Ok(crate::mail::threading::assign_thread(parsed, &thread_refs))
}
