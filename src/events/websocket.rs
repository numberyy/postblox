use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::RwLock;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use tokio::sync::broadcast;
use uuid::Uuid;

const CHANNEL_CAPACITY: usize = 100;
const MAX_CONNECTIONS: usize = 1_000;

pub struct WebSocketHub {
    channels: RwLock<HashMap<Uuid, broadcast::Sender<String>>>,
    total_connections: AtomicUsize,
}

impl Default for WebSocketHub {
    fn default() -> Self {
        Self::new()
    }
}

impl WebSocketHub {
    pub fn new() -> Self {
        Self {
            channels: RwLock::new(HashMap::new()),
            total_connections: AtomicUsize::new(0),
        }
    }

    pub fn subscribe(&self, org_id: Uuid) -> Option<broadcast::Receiver<String>> {
        // Acquire write lock first, then increment counter — prevents counter
        // leak if the lock panics (poisoned) between CAS and subscribe.
        let mut channels = self.channels.write().unwrap();
        let current = self.total_connections.load(Ordering::Relaxed);
        if current >= MAX_CONNECTIONS {
            return None;
        }
        self.total_connections.fetch_add(1, Ordering::Relaxed);
        let tx = channels
            .entry(org_id)
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0);
        Some(tx.subscribe())
    }

    pub fn unsubscribe(&self, org_id: Uuid) {
        self.total_connections.fetch_sub(1, Ordering::Relaxed);
        // Prune channel if no receivers remain — single write lock avoids TOCTOU
        let mut channels = self.channels.write().unwrap();
        if let std::collections::hash_map::Entry::Occupied(e) = channels.entry(org_id) {
            if e.get().receiver_count() == 0 {
                e.remove();
            }
        }
    }

    pub fn broadcast(&self, org_id: Uuid, event: &str, data: &serde_json::Value) {
        let channels = self.channels.read().unwrap();
        if let Some(tx) = channels.get(&org_id) {
            if tx.receiver_count() > 0 {
                let msg = serde_json::json!({
                    "event": event,
                    "data": data,
                });
                let _ = tx.send(msg.to_string());
            }
        }
    }

    pub async fn handle_ws(&self, mut socket: WebSocket, org_id: Uuid) {
        let mut rx = match self.subscribe(org_id) {
            Some(rx) => rx,
            None => {
                let _ = socket.send(Message::Close(None)).await;
                return;
            }
        };

        let mut ping_interval = tokio::time::interval(Duration::from_secs(30));
        let mut awaiting_pong = false;

        loop {
            tokio::select! {
                msg = rx.recv() => {
                    match msg {
                        Ok(text) => {
                            if socket.send(Message::Text(text.into())).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::debug!(org_id = %org_id, skipped = n, "ws client lagged");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = ping_interval.tick() => {
                    if awaiting_pong {
                        tracing::debug!(org_id = %org_id, "ws pong timeout");
                        break;
                    }
                    if socket.send(Message::Ping(vec![].into())).await.is_err() {
                        break;
                    }
                    awaiting_pong = true;
                }
                msg = socket.recv() => {
                    match msg {
                        Some(Ok(Message::Pong(_))) => { awaiting_pong = false; }
                        Some(Ok(Message::Close(_))) | None => break,
                        _ => {}
                    }
                }
            }
        }

        self.unsubscribe(org_id);
    }

    #[cfg(test)]
    pub fn connection_count(&self, org_id: Uuid) -> usize {
        let channels = self.channels.read().unwrap();
        channels
            .get(&org_id)
            .map(|tx| tx.receiver_count())
            .unwrap_or(0)
    }

    #[cfg(test)]
    pub fn total_connections(&self) -> usize {
        self.total_connections.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_hub_empty() {
        let hub = WebSocketHub::new();
        assert_eq!(hub.total_connections(), 0);
        assert_eq!(hub.connection_count(Uuid::new_v4()), 0);
    }

    #[test]
    fn test_subscribe_and_broadcast() {
        let hub = WebSocketHub::new();
        let org_id = Uuid::new_v4();
        let mut rx = hub.subscribe(org_id).unwrap();

        hub.broadcast(
            org_id,
            "message.received",
            &serde_json::json!({"message_id": "abc"}),
        );

        let msg = rx.try_recv().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(parsed["event"], "message.received");
        assert_eq!(parsed["data"]["message_id"], "abc");
    }

    #[test]
    fn test_broadcast_to_empty_org_is_noop() {
        let hub = WebSocketHub::new();
        let org_id = Uuid::new_v4();
        hub.broadcast(
            org_id,
            "message.received",
            &serde_json::json!({"id": "test"}),
        );
    }

    #[test]
    fn test_connection_count_tracks_subscribers() {
        let hub = WebSocketHub::new();
        let org_id = Uuid::new_v4();

        assert_eq!(hub.connection_count(org_id), 0);

        let _rx1 = hub.subscribe(org_id).unwrap();
        assert_eq!(hub.connection_count(org_id), 1);

        let _rx2 = hub.subscribe(org_id).unwrap();
        assert_eq!(hub.connection_count(org_id), 2);
    }

    #[test]
    fn test_total_connections_tracking() {
        let hub = WebSocketHub::new();
        let org1 = Uuid::new_v4();
        let org2 = Uuid::new_v4();

        let _rx1 = hub.subscribe(org1).unwrap();
        let _rx2 = hub.subscribe(org2).unwrap();
        assert_eq!(hub.total_connections(), 2);

        hub.unsubscribe(org1);
        assert_eq!(hub.total_connections(), 1);
    }

    #[test]
    fn test_multiple_orgs_isolated() {
        let hub = WebSocketHub::new();
        let org1 = Uuid::new_v4();
        let org2 = Uuid::new_v4();

        let mut rx1 = hub.subscribe(org1).unwrap();
        let mut rx2 = hub.subscribe(org2).unwrap();

        hub.broadcast(
            org1,
            "message.received",
            &serde_json::json!({"for": "org1"}),
        );

        assert!(rx1.try_recv().is_ok());
        assert!(rx2.try_recv().is_err());
    }

    #[test]
    fn test_broadcast_event_format() {
        let hub = WebSocketHub::new();
        let org_id = Uuid::new_v4();
        let mut rx = hub.subscribe(org_id).unwrap();

        let approval_id = Uuid::new_v4();
        let inbox_id = Uuid::new_v4();
        hub.broadcast(
            org_id,
            "approval.requested",
            &serde_json::json!({
                "approval_id": approval_id.to_string(),
                "inbox_id": inbox_id.to_string(),
                "subject": "Test email",
            }),
        );

        let msg = rx.try_recv().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(parsed["event"], "approval.requested");
        assert!(parsed["data"]["approval_id"].is_string());
        assert!(parsed["data"]["inbox_id"].is_string());
        assert_eq!(parsed["data"]["subject"], "Test email");
    }

    #[test]
    fn test_subscribe_same_org_shares_channel() {
        let hub = WebSocketHub::new();
        let org_id = Uuid::new_v4();

        let mut rx1 = hub.subscribe(org_id).unwrap();
        let mut rx2 = hub.subscribe(org_id).unwrap();

        hub.broadcast(org_id, "message.sent", &serde_json::json!({"id": "shared"}));

        assert!(rx1.try_recv().is_ok());
        assert!(rx2.try_recv().is_ok());
    }

    #[test]
    fn test_connection_limit_enforced() {
        let hub = WebSocketHub::new();
        let mut receivers = Vec::new();
        for _ in 0..MAX_CONNECTIONS {
            let org_id = Uuid::new_v4();
            receivers.push(hub.subscribe(org_id).unwrap());
        }
        assert_eq!(hub.total_connections(), MAX_CONNECTIONS);
        // Next subscribe should be rejected
        assert!(hub.subscribe(Uuid::new_v4()).is_none());
    }

    #[test]
    fn test_channel_pruned_after_last_unsubscribe() {
        let hub = WebSocketHub::new();
        let org_id = Uuid::new_v4();

        let rx = hub.subscribe(org_id).unwrap();
        assert_eq!(hub.connection_count(org_id), 1);

        drop(rx);
        hub.unsubscribe(org_id);

        // Channel should be pruned since receiver was dropped
        assert_eq!(hub.connection_count(org_id), 0);
        let channels = hub.channels.read().unwrap();
        assert!(!channels.contains_key(&org_id));
    }

    #[test]
    fn test_dropped_receiver_does_not_block_broadcast() {
        let hub = WebSocketHub::new();
        let org_id = Uuid::new_v4();

        let rx = hub.subscribe(org_id).unwrap();
        drop(rx);
        hub.unsubscribe(org_id);

        hub.broadcast(
            org_id,
            "message.received",
            &serde_json::json!({"id": "test"}),
        );
    }
}
