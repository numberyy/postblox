//! Per-account sync state transitions surfaced on the IPC hub.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ipc::{Hub, Topic};
use crate::models::AccountId;

/// Wire enum for the `sync.state` topic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncState {
    /// No sync activity is in progress for the account.
    Idle,
    /// IDLE worker is waiting on the server for a change notification.
    Polling,
    /// Reconciler is pulling new messages right now.
    Syncing,
    /// Last sync attempt failed; details are in [`SyncStateEvent::last_error`].
    Error,
}

/// Payload published on `Topic::SyncState`. `last_error` is `Some` when
/// `state == SyncState::Error`; otherwise `None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncStateEvent {
    /// Account whose sync state changed.
    pub account_id: AccountId,
    /// New sync state.
    pub state: SyncState,
    /// Most recent error message when `state == SyncState::Error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl SyncStateEvent {
    /// Build a new `sync.state` event payload.
    pub fn new(account_id: AccountId, state: SyncState, last_error: Option<String>) -> Self {
        Self {
            account_id,
            state,
            last_error,
        }
    }
}

/// Publish a transition. Logs but never panics if serialization fails.
pub async fn publish_sync_state(hub: &Arc<Hub>, event: SyncStateEvent) {
    match serde_json::to_value(&event) {
        Ok(payload) => hub.publish(Topic::SyncState, payload).await,
        Err(error) => {
            tracing::warn!(error = %error, "failed to encode sync.state payload");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_sync_state_serializes_lowercase() {
        let value = serde_json::to_value(SyncState::Polling).unwrap();
        assert_eq!(value, json!("polling"));
    }

    #[test]
    fn test_event_omits_last_error_when_none() {
        let account_id = AccountId::new();
        let event = SyncStateEvent::new(account_id, SyncState::Idle, None);
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["state"], json!("idle"));
        assert!(value.get("last_error").is_none());
    }

    #[test]
    fn test_event_carries_last_error_on_error_state() {
        let account_id = AccountId::new();
        let event = SyncStateEvent::new(account_id, SyncState::Error, Some("login refused".into()));
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["state"], json!("error"));
        assert_eq!(value["last_error"], json!("login refused"));
    }

    #[tokio::test]
    async fn test_publish_sync_state_emits_to_hub() {
        let hub = Arc::new(Hub::new());
        let mut rx = hub.subscribe(Topic::SyncState).await;
        let account_id = AccountId::new();

        publish_sync_state(
            &hub,
            SyncStateEvent::new(account_id, SyncState::Syncing, None),
        )
        .await;

        let payload = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .unwrap()
            .unwrap();
        let decoded: SyncStateEvent = serde_json::from_value((*payload).clone()).unwrap();
        assert_eq!(decoded.account_id, account_id);
        assert_eq!(decoded.state, SyncState::Syncing);
        assert_eq!(decoded.last_error, None);
    }
}
