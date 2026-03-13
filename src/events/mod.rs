pub mod webhooks;
pub mod websocket;

use sqlx::PgPool;
use uuid::Uuid;

pub const KNOWN_EVENTS: &[&str] = &[
    "message.received",
    "message.sent",
    "message.classified",
    "approval.requested",
    "trust.changed",
];

pub enum PostbloxEvent {
    MessageReceived {
        message_id: Uuid,
        inbox_id: Uuid,
    },
    MessageSent {
        message_id: Uuid,
        inbox_id: Uuid,
    },
    MessageClassified {
        message_id: Uuid,
        inbox_id: Uuid,
    },
    ApprovalRequested {
        message_id: Uuid,
        inbox_id: Uuid,
        approval_id: Uuid,
    },
    TrustChanged {
        inbox_id: Uuid,
        new_mode: crate::models::SendMode,
        approved_count: i32,
    },
}

impl PostbloxEvent {
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::MessageReceived { .. } => "message.received",
            Self::MessageSent { .. } => "message.sent",
            Self::MessageClassified { .. } => "message.classified",
            Self::ApprovalRequested { .. } => "approval.requested",
            Self::TrustChanged { .. } => "trust.changed",
        }
    }

    pub fn data(&self) -> serde_json::Value {
        match self {
            Self::MessageReceived {
                message_id,
                inbox_id,
            }
            | Self::MessageSent {
                message_id,
                inbox_id,
            }
            | Self::MessageClassified {
                message_id,
                inbox_id,
            } => serde_json::json!({
                "message_id": message_id,
                "inbox_id": inbox_id,
            }),
            Self::ApprovalRequested {
                message_id,
                inbox_id,
                approval_id,
            } => serde_json::json!({
                "message_id": message_id,
                "inbox_id": inbox_id,
                "approval_id": approval_id,
            }),
            Self::TrustChanged {
                inbox_id,
                new_mode,
                approved_count,
            } => serde_json::json!({
                "inbox_id": inbox_id,
                "new_mode": new_mode.to_string(),
                "approved_count": approved_count,
            }),
        }
    }
}

pub async fn audit(
    pool: &PgPool,
    org_id: Uuid,
    inbox_id: Option<Uuid>,
    action: crate::models::AuditAction,
    actor: &str,
    details: serde_json::Value,
) {
    if let Err(e) =
        crate::db::audit::create_entry(pool, org_id, inbox_id, &action.to_string(), actor, details)
            .await
    {
        tracing::error!("failed to create audit entry: {e}");
    }
}

pub async fn dispatch(
    pool: &PgPool,
    org_id: Uuid,
    event: PostbloxEvent,
    client: &reqwest::Client,
    hooks: &[crate::hooks::HookConfig],
    ws_hub: &websocket::WebSocketHub,
) {
    // Fire notifications for relevant events
    match &event {
        PostbloxEvent::ApprovalRequested {
            message_id,
            inbox_id,
            approval_id,
        } => {
            crate::notifications::notify_org(
                pool,
                org_id,
                "Approval Required",
                &format!(
                    "Message {message_id} from inbox {inbox_id} needs approval (approval {approval_id})"
                ),
                client,
            )
            .await;
        }
        PostbloxEvent::TrustChanged {
            inbox_id,
            new_mode,
            approved_count,
        } => {
            crate::notifications::notify_org(
                pool,
                org_id,
                "Trust Level Changed",
                &format!(
                    "Inbox {inbox_id} auto-upgraded to {new_mode} after {approved_count} approved sends"
                ),
                client,
            )
            .await;
        }
        _ => {}
    }

    let event_name = event.event_name();
    let data = event.data();

    let wh_list = match crate::db::webhooks::list_active_for_event(pool, org_id, event_name).await {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("failed to query webhooks for {event_name}: {e}");
            ws_hub.broadcast(org_id, event_name, &data);
            crate::hooks::run_event_hooks(hooks, event_name, data);
            return;
        }
    };

    if wh_list.is_empty()
        && hooks.iter().all(|h| h.event != event_name)
        && ws_hub.connection_count(org_id) == 0
    {
        return;
    }

    if wh_list.len() > 20 {
        tracing::warn!(org_id = %org_id, count = wh_list.len(), "exceeds webhook concurrency limit, delivering first 20");
    }

    for wh in wh_list.into_iter().take(20) {
        let client = client.clone();
        let name = event_name.to_string();
        let payload = data.clone();
        tokio::spawn(async move {
            if let Err(e) = webhooks::deliver(&client, &wh.url, &wh.secret, &name, &payload).await {
                tracing::warn!(webhook_id = %wh.id, url = %wh.url, "webhook delivery failed: {e}");
            }
        });
    }

    ws_hub.broadcast(org_id, event_name, &data);

    crate::hooks::run_event_hooks(hooks, event_name, data);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_name_message_received() {
        let event = PostbloxEvent::MessageReceived {
            message_id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
        };
        assert_eq!(event.event_name(), "message.received");
    }

    #[test]
    fn test_event_name_message_sent() {
        let event = PostbloxEvent::MessageSent {
            message_id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
        };
        assert_eq!(event.event_name(), "message.sent");
    }

    #[test]
    fn test_event_data_contains_ids() {
        let msg_id = Uuid::new_v4();
        let inbox_id = Uuid::new_v4();
        let event = PostbloxEvent::MessageReceived {
            message_id: msg_id,
            inbox_id,
        };
        let data = event.data();
        assert_eq!(data["message_id"], msg_id.to_string());
        assert_eq!(data["inbox_id"], inbox_id.to_string());
    }

    #[test]
    fn test_event_data_sent_contains_ids() {
        let msg_id = Uuid::new_v4();
        let inbox_id = Uuid::new_v4();
        let event = PostbloxEvent::MessageSent {
            message_id: msg_id,
            inbox_id,
        };
        let data = event.data();
        assert_eq!(data["message_id"], msg_id.to_string());
        assert_eq!(data["inbox_id"], inbox_id.to_string());
    }

    #[test]
    fn test_event_name_approval_requested() {
        let event = PostbloxEvent::ApprovalRequested {
            message_id: Uuid::new_v4(),
            inbox_id: Uuid::new_v4(),
            approval_id: Uuid::new_v4(),
        };
        assert_eq!(event.event_name(), "approval.requested");
    }

    #[test]
    fn test_event_data_approval_requested_contains_ids() {
        let msg_id = Uuid::new_v4();
        let inbox_id = Uuid::new_v4();
        let approval_id = Uuid::new_v4();
        let event = PostbloxEvent::ApprovalRequested {
            message_id: msg_id,
            inbox_id,
            approval_id,
        };
        let data = event.data();
        assert_eq!(data["message_id"], msg_id.to_string());
        assert_eq!(data["inbox_id"], inbox_id.to_string());
        assert_eq!(data["approval_id"], approval_id.to_string());
    }

    #[test]
    fn test_known_events_includes_approval_requested() {
        assert!(KNOWN_EVENTS.contains(&"approval.requested"));
    }

    #[test]
    fn test_event_name_trust_changed() {
        let event = PostbloxEvent::TrustChanged {
            inbox_id: Uuid::new_v4(),
            new_mode: crate::models::SendMode::AutoApprove,
            approved_count: 10,
        };
        assert_eq!(event.event_name(), "trust.changed");
    }

    #[test]
    fn test_event_data_trust_changed_contains_fields() {
        let inbox_id = Uuid::new_v4();
        let event = PostbloxEvent::TrustChanged {
            inbox_id,
            new_mode: crate::models::SendMode::AutoApprove,
            approved_count: 15,
        };
        let data = event.data();
        assert_eq!(data["inbox_id"], inbox_id.to_string());
        assert_eq!(data["new_mode"], "auto_approve");
        assert_eq!(data["approved_count"], 15);
    }

    #[test]
    fn test_known_events_includes_trust_changed() {
        assert!(KNOWN_EVENTS.contains(&"trust.changed"));
    }
}
