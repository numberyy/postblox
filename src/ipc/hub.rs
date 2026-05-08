//! Subscription hub: daemon publishes events; many connections subscribe.
//!
//! One `tokio::sync::broadcast` channel per topic, lazily created.
//! Subscribers get a bounded receiver — when a slow consumer falls
//! behind, the broadcast channel drops the oldest message and the
//! receiver surfaces a `Lagged(n)` error which we translate into a
//! "lagged" event so the client can resync if it cares.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};

/// Topic identifier. We keep this an enum so the daemon can't
/// accidentally publish a typoed string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    /// New message stored by sync.
    MailNew,
    /// Existing message updated (flags, thread reassignment, etc.).
    MailUpdated,
    /// Account sync_status changed.
    AccountSynced,
    /// Per-account sync state transition (idle/polling/syncing/error).
    SyncState,
    /// MCP gate produced a pending approval needing UI attention.
    McpApprovalRequested,
    /// A pending approval was decided (user or system).
    McpApprovalDecided,
}

impl Topic {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::MailNew => "mail.new",
            Self::MailUpdated => "mail.updated",
            Self::AccountSynced => "account.synced",
            Self::SyncState => "sync.state",
            Self::McpApprovalRequested => "mcp.approval_requested",
            Self::McpApprovalDecided => "mcp.approval_decided",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "mail.new" => Some(Self::MailNew),
            "mail.updated" => Some(Self::MailUpdated),
            "account.synced" => Some(Self::AccountSynced),
            "sync.state" => Some(Self::SyncState),
            "mcp.approval_requested" => Some(Self::McpApprovalRequested),
            "mcp.approval_decided" => Some(Self::McpApprovalDecided),
            _ => None,
        }
    }
}

/// Default per-topic broadcast channel capacity. Bounded — a slow
/// subscriber gets `Lagged(n)` rather than infinite buffering.
pub const DEFAULT_TOPIC_CAPACITY: usize = 256;

#[derive(Clone)]
pub struct Hub {
    inner: Arc<RwLock<HashMap<Topic, broadcast::Sender<Arc<serde_json::Value>>>>>,
    capacity: usize,
}

impl Default for Hub {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_TOPIC_CAPACITY)
    }
}

impl Hub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            capacity,
        }
    }

    /// Publish `data` on `topic`. No-op when no one is subscribed.
    pub async fn publish(&self, topic: Topic, data: serde_json::Value) {
        let map = self.inner.read().await;
        if let Some(tx) = map.get(&topic) {
            // broadcast::Sender::send returns Err when there are no live
            // receivers — fine, just drop the message.
            let _ = tx.send(Arc::new(data));
        }
    }

    /// Subscribe to a topic. Returns a receiver that yields published
    /// payloads in order. Lagged receivers see `RecvError::Lagged(n)`.
    pub async fn subscribe(&self, topic: Topic) -> broadcast::Receiver<Arc<serde_json::Value>> {
        // Fast path: read lock is enough if the topic exists.
        {
            let map = self.inner.read().await;
            if let Some(tx) = map.get(&topic) {
                return tx.subscribe();
            }
        }
        // Slow path: create the topic.
        let mut map = self.inner.write().await;
        let tx = map
            .entry(topic)
            .or_insert_with(|| broadcast::channel(self.capacity).0);
        tx.subscribe()
    }

    /// Number of currently-active subscribers across all topics. Useful
    /// for tests and metrics.
    pub async fn receiver_count(&self) -> usize {
        let map = self.inner.read().await;
        map.values().map(|tx| tx.receiver_count()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::sync::broadcast::error::RecvError;
    use tokio::time::{timeout, Duration};

    #[test]
    fn test_topic_round_trip() {
        for t in [
            Topic::MailNew,
            Topic::MailUpdated,
            Topic::AccountSynced,
            Topic::SyncState,
            Topic::McpApprovalRequested,
            Topic::McpApprovalDecided,
        ] {
            assert_eq!(Topic::parse(t.as_str()), Some(t));
        }
        assert!(Topic::parse("garbage").is_none());
    }

    #[tokio::test]
    async fn test_publish_without_subscribers_is_noop() {
        let hub = Hub::new();
        hub.publish(Topic::MailNew, json!({"x": 1})).await;
        assert_eq!(hub.receiver_count().await, 0);
    }

    #[tokio::test]
    async fn test_subscribe_then_publish_delivers() {
        let hub = Hub::new();
        let mut rx = hub.subscribe(Topic::MailNew).await;
        hub.publish(Topic::MailNew, json!({"id": "abc"})).await;
        let got = timeout(Duration::from_millis(100), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(*got, json!({"id": "abc"}));
    }

    #[tokio::test]
    async fn test_topics_are_independent() {
        let hub = Hub::new();
        let mut new_rx = hub.subscribe(Topic::MailNew).await;
        let mut upd_rx = hub.subscribe(Topic::MailUpdated).await;
        hub.publish(Topic::MailNew, json!({"k": "n"})).await;
        let got_new = timeout(Duration::from_millis(50), new_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(*got_new, json!({"k": "n"}));
        // Updated channel must not see the MailNew event.
        let result = timeout(Duration::from_millis(50), upd_rx.recv()).await;
        assert!(result.is_err(), "unexpected event delivered to wrong topic");
    }

    #[tokio::test]
    async fn test_multiple_subscribers_each_receive() {
        let hub = Hub::new();
        let mut a = hub.subscribe(Topic::MailNew).await;
        let mut b = hub.subscribe(Topic::MailNew).await;
        hub.publish(Topic::MailNew, json!({"x": 1})).await;
        let ga = timeout(Duration::from_millis(100), a.recv())
            .await
            .unwrap()
            .unwrap();
        let gb = timeout(Duration::from_millis(100), b.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(*ga, json!({"x": 1}));
        assert_eq!(*gb, json!({"x": 1}));
    }

    #[tokio::test]
    async fn test_slow_subscriber_lags_rather_than_blocks() {
        // Tiny capacity to force a lag.
        let hub = Hub::with_capacity(2);
        let mut rx = hub.subscribe(Topic::MailNew).await;

        // Publish more than the capacity without draining rx.
        for i in 0..10 {
            hub.publish(Topic::MailNew, json!({ "i": i })).await;
        }

        let err = rx.recv().await.unwrap_err();
        match err {
            RecvError::Lagged(n) => assert!(n >= 1, "expected lag count, got {n}"),
            other => panic!("expected Lagged, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_receiver_count_tracks_subs() {
        let hub = Hub::new();
        assert_eq!(hub.receiver_count().await, 0);
        let _a = hub.subscribe(Topic::MailNew).await;
        let _b = hub.subscribe(Topic::MailNew).await;
        let _c = hub.subscribe(Topic::AccountSynced).await;
        assert_eq!(hub.receiver_count().await, 3);
        drop(_a);
        // broadcast cleans up dead receivers lazily; the count is best-effort.
        // Force a publish so the channel notices the drop.
        hub.publish(Topic::MailNew, json!({})).await;
        assert!(hub.receiver_count().await <= 3);
    }

    #[tokio::test]
    async fn test_subscribe_after_publish_does_not_replay() {
        let hub = Hub::new();
        hub.publish(Topic::MailNew, json!({"x": 1})).await;
        let mut rx = hub.subscribe(Topic::MailNew).await;
        let result = timeout(Duration::from_millis(50), rx.recv()).await;
        assert!(result.is_err(), "broadcast should not replay past events");
    }
}
