use uuid::Uuid;

use super::error::ApiError;
use super::AppState;
use crate::models::Message;

pub struct DeliveryParams<'a> {
    pub from: &'a str,
    pub to: &'a [String],
    pub cc: &'a [String],
    pub subject: &'a str,
    pub text_body: Option<&'a str>,
    pub html_body: Option<&'a str>,
    pub message_id_header: &'a str,
}

pub async fn deliver_message(
    state: &AppState,
    org_id: Uuid,
    inbox: &crate::models::Inbox,
    msg_id: Uuid,
    params: &DeliveryParams<'_>,
) -> Result<(), ApiError> {
    if !inbox.active {
        return Err(ApiError::Forbidden(
            "inbox is disabled due to excessive bounces".into(),
        ));
    }

    let inbox_id = inbox.id;
    let mime_message_id = if params.message_id_header.starts_with('<') {
        params.message_id_header.to_string()
    } else {
        format!("<{}>", params.message_id_header)
    };

    let raw_mime = crate::mail::builder::build_mime(
        params.from,
        params.to,
        params.cc,
        params.subject,
        params.text_body,
        params.html_body,
        &mime_message_id,
    );

    if let Some(ref stalwart) = state.stalwart {
        let to_refs: Vec<&str> = params.to.iter().map(|s| s.as_str()).collect();
        if let Err(e) = stalwart
            .submit_message(params.from, &to_refs, raw_mime)
            .await
        {
            tracing::error!("stalwart submission failed: {e}");
            return Err(ApiError::Internal("email delivery failed".into()));
        }
    } else {
        return Err(ApiError::Internal(
            "email delivery unavailable: stalwart not configured".into(),
        ));
    }

    let pool = state.pool.clone();
    let webhook_client = state.webhook_client.clone();
    let hooks = state.hooks.clone();
    let ws_hub = state.ws_hub.clone();
    tokio::spawn(async move {
        crate::events::dispatch(
            &pool,
            org_id,
            crate::events::PostbloxEvent::MessageSent {
                message_id: msg_id,
                inbox_id,
            },
            &webhook_client,
            &hooks,
            &ws_hub,
        )
        .await;
    });

    Ok(())
}

pub fn extract_addrs(msg: &Message) -> (Vec<String>, Vec<String>) {
    let to: Vec<String> = msg
        .to_addrs
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let cc: Vec<String> = msg
        .cc_addrs
        .as_ref()
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    (to, cc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_addrs_basic() {
        let msg = Message {
            id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            thread_id: None,
            message_id_header: None,
            in_reply_to: None,
            references_header: None,
            from_addr: "a@b.com".into(),
            to_addrs: serde_json::json!(["user@example.com", "other@example.com"]),
            cc_addrs: Some(serde_json::json!(["cc@example.com"])),
            subject: None,
            text_body: None,
            html_body: None,
            extracted_text: None,
            direction: crate::models::Direction::Outbound,
            raw_headers: None,
            created_at: chrono::Utc::now(),
            slop_score: None,
            slop_signals: None,
            category: None,
            priority: None,
            triage_status: None,
            requires_action: None,
        };
        let (to, cc) = extract_addrs(&msg);
        assert_eq!(to, vec!["user@example.com", "other@example.com"]);
        assert_eq!(cc, vec!["cc@example.com"]);
    }

    #[test]
    fn test_extract_addrs_empty() {
        let msg = Message {
            id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            thread_id: None,
            message_id_header: None,
            in_reply_to: None,
            references_header: None,
            from_addr: "a@b.com".into(),
            to_addrs: serde_json::json!([]),
            cc_addrs: None,
            subject: None,
            text_body: None,
            html_body: None,
            extracted_text: None,
            direction: crate::models::Direction::Outbound,
            raw_headers: None,
            created_at: chrono::Utc::now(),
            slop_score: None,
            slop_signals: None,
            category: None,
            priority: None,
            triage_status: None,
            requires_action: None,
        };
        let (to, cc) = extract_addrs(&msg);
        assert!(to.is_empty());
        assert!(cc.is_empty());
    }

    #[test]
    fn test_extract_addrs_non_string_values_filtered() {
        let msg = Message {
            id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            thread_id: None,
            message_id_header: None,
            in_reply_to: None,
            references_header: None,
            from_addr: "a@b.com".into(),
            to_addrs: serde_json::json!(["good@a.com", 123, null, "ok@b.com"]),
            cc_addrs: None,
            subject: None,
            text_body: None,
            html_body: None,
            extracted_text: None,
            direction: crate::models::Direction::Outbound,
            raw_headers: None,
            created_at: chrono::Utc::now(),
            slop_score: None,
            slop_signals: None,
            category: None,
            priority: None,
            triage_status: None,
            requires_action: None,
        };
        let (to, _) = extract_addrs(&msg);
        assert_eq!(to, vec!["good@a.com", "ok@b.com"]);
    }
}
