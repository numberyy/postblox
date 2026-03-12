pub mod webhooks;

use sqlx::PgPool;
use uuid::Uuid;

pub const KNOWN_EVENTS: &[&str] = &["message.received", "message.sent"];

pub enum PostbloxEvent {
    MessageReceived { message_id: Uuid, inbox_id: Uuid },
    MessageSent { message_id: Uuid, inbox_id: Uuid },
}

impl PostbloxEvent {
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::MessageReceived { .. } => "message.received",
            Self::MessageSent { .. } => "message.sent",
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
            } => serde_json::json!({
                "message_id": message_id,
                "inbox_id": inbox_id,
            }),
        }
    }
}

pub async fn dispatch(pool: &PgPool, org_id: Uuid, event: PostbloxEvent, client: &reqwest::Client) {
    let event_name = event.event_name();
    let data = event.data();

    let hooks = match crate::db::webhooks::list_active_for_event(pool, org_id, event_name).await {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("failed to query webhooks for {event_name}: {e}");
            return;
        }
    };

    if hooks.is_empty() {
        return;
    }

    if hooks.len() > 20 {
        tracing::warn!(org_id = %org_id, count = hooks.len(), "exceeds webhook concurrency limit, delivering first 20");
    }

    for wh in hooks.into_iter().take(20) {
        let client = client.clone();
        let name = event_name.to_string();
        let payload = data.clone();
        tokio::spawn(async move {
            if let Err(e) = webhooks::deliver(&client, &wh.url, &wh.secret, &name, &payload).await {
                tracing::warn!(webhook_id = %wh.id, url = %wh.url, "webhook delivery failed: {e}");
            }
        });
    }
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
}
