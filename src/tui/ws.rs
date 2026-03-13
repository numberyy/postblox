use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum WsEvent {
    MessageReceived {
        #[allow(dead_code)]
        message_id: Uuid,
        inbox_id: Uuid,
    },
    ApprovalRequested {
        #[allow(dead_code)]
        message_id: Uuid,
        #[allow(dead_code)]
        inbox_id: Uuid,
        #[allow(dead_code)]
        approval_id: Uuid,
    },
    TrustChanged {
        #[allow(dead_code)]
        inbox_id: Uuid,
        #[allow(dead_code)]
        new_mode: String,
        #[allow(dead_code)]
        approved_count: i64,
    },
    Connected,
    Disconnected,
}

#[derive(Debug, Deserialize)]
struct WsMessage {
    event: String,
    data: serde_json::Value,
}

pub async fn run(
    ws_url: String,
    tx: mpsc::Sender<WsEvent>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut backoff_ms: u64 = 1_000;
    const MAX_BACKOFF_MS: u64 = 30_000;

    loop {
        if *shutdown.borrow() {
            return;
        }

        let kind = match connect_and_listen(&ws_url, &tx, &mut shutdown).await {
            Ok(()) => return, // clean shutdown
            Err((kind, e)) => {
                tracing::warn!("ws disconnected: {e}");
                // Receiver closed means TUI is shutting down; ignore send error.
                let _ = tx.send(WsEvent::Disconnected).await;
                kind
            }
        };

        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)) => {}
            _ = shutdown.changed() => return,
        }

        match kind {
            DisconnectKind::WasConnected => backoff_ms = 1_000,
            DisconnectKind::NeverConnected => {
                backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
            }
        }
    }
}

type WsError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DisconnectKind {
    NeverConnected,
    WasConnected,
}

async fn connect_and_listen(
    url: &str,
    tx: &mpsc::Sender<WsEvent>,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<(), (DisconnectKind, WsError)> {
    let (mut ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| (DisconnectKind::NeverConnected, e.into()))?;
    // Receiver closed means TUI is shutting down; ignore send error.
    let _ = tx.send(WsEvent::Connected).await;

    loop {
        tokio::select! {
            msg = ws.next() => {
                match msg {
                    Some(Ok(tungstenite::Message::Text(text))) => {
                        if let Some(event) = parse_ws_event(&text) {
                            // Receiver closed means TUI is shutting down; ignore send error.
                            let _ = tx.send(event).await;
                        }
                    }
                    Some(Ok(tungstenite::Message::Ping(data))) => {
                        ws.send(tungstenite::Message::Pong(data)).await.map_err(|e| (DisconnectKind::WasConnected, e.into()))?;
                    }
                    Some(Ok(tungstenite::Message::Close(_))) | None => break,
                    Some(Err(e)) => return Err((DisconnectKind::WasConnected, e.into())),
                    _ => {}
                }
            }
            _ = shutdown.changed() => {
                // Shutdown requested; ignore close errors.
                let _ = ws.close(None).await;
                return Ok(());
            }
        }
    }

    Ok(())
}

fn parse_ws_event(text: &str) -> Option<WsEvent> {
    let msg: WsMessage = serde_json::from_str(text).ok()?;
    match msg.event.as_str() {
        "message.received" | "message.sent" | "message.classified" => {
            let message_id = parse_uuid(&msg.data, "message_id")?;
            let inbox_id = parse_uuid(&msg.data, "inbox_id")?;
            Some(WsEvent::MessageReceived {
                message_id,
                inbox_id,
            })
        }
        "approval.requested" => {
            let message_id = parse_uuid(&msg.data, "message_id")?;
            let inbox_id = parse_uuid(&msg.data, "inbox_id")?;
            let approval_id = parse_uuid(&msg.data, "approval_id")?;
            Some(WsEvent::ApprovalRequested {
                message_id,
                inbox_id,
                approval_id,
            })
        }
        "trust.changed" => {
            let inbox_id = parse_uuid(&msg.data, "inbox_id")?;
            let new_mode = msg.data["new_mode"].as_str()?.to_string();
            let approved_count = msg.data["approved_count"].as_i64()?;
            Some(WsEvent::TrustChanged {
                inbox_id,
                new_mode,
                approved_count,
            })
        }
        _ => None,
    }
}

fn parse_uuid(data: &serde_json::Value, key: &str) -> Option<Uuid> {
    data[key].as_str()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_message_received() {
        let json = r#"{"event":"message.received","data":{"message_id":"550e8400-e29b-41d4-a716-446655440000","inbox_id":"660e8400-e29b-41d4-a716-446655440000"}}"#;
        let event = parse_ws_event(json).unwrap();
        match event {
            WsEvent::MessageReceived {
                message_id,
                inbox_id,
            } => {
                assert_eq!(
                    message_id.to_string(),
                    "550e8400-e29b-41d4-a716-446655440000"
                );
                assert_eq!(inbox_id.to_string(), "660e8400-e29b-41d4-a716-446655440000");
            }
            _ => panic!("expected MessageReceived"),
        }
    }

    #[test]
    fn test_parse_message_sent_as_received() {
        let json = r#"{"event":"message.sent","data":{"message_id":"550e8400-e29b-41d4-a716-446655440000","inbox_id":"660e8400-e29b-41d4-a716-446655440000"}}"#;
        let event = parse_ws_event(json).unwrap();
        assert!(matches!(event, WsEvent::MessageReceived { .. }));
    }

    #[test]
    fn test_parse_approval_requested() {
        let json = r#"{"event":"approval.requested","data":{"message_id":"550e8400-e29b-41d4-a716-446655440000","inbox_id":"660e8400-e29b-41d4-a716-446655440000","approval_id":"770e8400-e29b-41d4-a716-446655440000"}}"#;
        let event = parse_ws_event(json).unwrap();
        match event {
            WsEvent::ApprovalRequested { approval_id, .. } => {
                assert_eq!(
                    approval_id.to_string(),
                    "770e8400-e29b-41d4-a716-446655440000"
                );
            }
            _ => panic!("expected ApprovalRequested"),
        }
    }

    #[test]
    fn test_parse_trust_changed() {
        let json = r#"{"event":"trust.changed","data":{"inbox_id":"550e8400-e29b-41d4-a716-446655440000","new_mode":"auto_approve","approved_count":10}}"#;
        let event = parse_ws_event(json).unwrap();
        match event {
            WsEvent::TrustChanged {
                new_mode,
                approved_count,
                ..
            } => {
                assert_eq!(new_mode, "auto_approve");
                assert_eq!(approved_count, 10);
            }
            _ => panic!("expected TrustChanged"),
        }
    }

    #[test]
    fn test_parse_unknown_event() {
        let json = r#"{"event":"unknown.event","data":{}}"#;
        assert!(parse_ws_event(json).is_none());
    }

    #[test]
    fn test_parse_invalid_json() {
        assert!(parse_ws_event("not json").is_none());
    }

    #[test]
    fn test_parse_missing_uuid_field() {
        let json = r#"{"event":"message.received","data":{"message_id":"550e8400-e29b-41d4-a716-446655440000"}}"#;
        assert!(parse_ws_event(json).is_none());
    }

    #[test]
    fn test_parse_invalid_uuid() {
        let json = r#"{"event":"message.received","data":{"message_id":"not-a-uuid","inbox_id":"660e8400-e29b-41d4-a716-446655440000"}}"#;
        assert!(parse_ws_event(json).is_none());
    }

    #[test]
    fn test_parse_classified_as_received() {
        let json = r#"{"event":"message.classified","data":{"message_id":"550e8400-e29b-41d4-a716-446655440000","inbox_id":"660e8400-e29b-41d4-a716-446655440000"}}"#;
        let event = parse_ws_event(json).unwrap();
        assert!(matches!(event, WsEvent::MessageReceived { .. }));
    }

    #[test]
    fn test_parse_uuid_helper() {
        let data = serde_json::json!({"id": "550e8400-e29b-41d4-a716-446655440000"});
        assert!(parse_uuid(&data, "id").is_some());
        assert!(parse_uuid(&data, "missing").is_none());
    }
}
