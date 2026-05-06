//! Long-running polling sync worker for one account/folder pair.

use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::imap::{ImapError, ImapSync};
use crate::ipc::Hub;
use crate::secrets::Secret;

use super::{reconcile_folder, SyncError};

#[derive(Debug, Clone, Copy)]
pub struct WorkerConfig {
    pub poll_interval: Duration,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(60),
            initial_backoff: Duration::from_secs(5),
            max_backoff: Duration::from_secs(60),
        }
    }
}

pub(crate) struct PollingWorker {
    pub(crate) pool: SqlitePool,
    pub(crate) hub: Arc<Hub>,
    pub(crate) imap: Arc<dyn ImapSync>,
    pub(crate) account_id: Uuid,
    pub(crate) folder_name: String,
    pub(crate) secret: Secret,
    pub(crate) cancel: CancellationToken,
    pub(crate) config: WorkerConfig,
}

pub(crate) async fn run_polling_worker(worker: PollingWorker) {
    let PollingWorker {
        pool,
        hub,
        imap,
        account_id,
        folder_name,
        secret,
        cancel,
        config,
    } = worker;
    let mut backoff = config.initial_backoff;

    loop {
        if cancel.is_cancelled() {
            return;
        }

        let result = tokio::select! {
            _ = cancel.cancelled() => return,
            result = reconcile_folder(
                &pool,
                &hub,
                imap.as_ref(),
                account_id,
                &folder_name,
                &secret,
            ) => result,
        };

        match result {
            Ok(report) => {
                tracing::debug!(
                    %account_id,
                    folder_name = %folder_name,
                    inserted = report.inserted,
                    wiped = report.wiped,
                    "sync worker reconciled folder"
                );
                backoff = config.initial_backoff;
                if sleep_or_cancel(config.poll_interval, &cancel).await {
                    return;
                }
            }
            Err(err) if is_auth_error(&err) => {
                tracing::warn!(
                    %account_id,
                    folder_name = %folder_name,
                    error = %err,
                    "sync worker stopped after authentication failure"
                );
                return;
            }
            Err(err) => {
                tracing::warn!(
                    %account_id,
                    folder_name = %folder_name,
                    error = %err,
                    retry_in_ms = backoff.as_millis(),
                    "sync worker reconcile failed; retrying"
                );
                if sleep_or_cancel(backoff, &cancel).await {
                    return;
                }
                backoff = next_backoff(backoff, config.max_backoff);
            }
        }
    }
}

fn is_auth_error(err: &SyncError) -> bool {
    matches!(
        err,
        SyncError::Imap(ImapError::Auth(_)) | SyncError::MissingCredentials
    )
}

fn next_backoff(current: Duration, max: Duration) -> Duration {
    current.saturating_mul(2).min(max)
}

async fn sleep_or_cancel(duration: Duration, cancel: &CancellationToken) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => true,
        _ = tokio::time::sleep(duration) => false,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use tokio::task::JoinHandle;
    use tokio::time::{timeout, Duration};

    use crate::db::{accounts, folders};
    use crate::imap::FolderSync;
    use crate::models::{AuthKind, FolderRole};

    use super::*;

    #[derive(Debug, Clone)]
    enum Outcome {
        Ok,
        Protocol,
        Auth,
    }

    struct ScriptedSync {
        calls: AtomicUsize,
        outcomes: Mutex<VecDeque<Outcome>>,
    }

    impl ScriptedSync {
        fn new(outcomes: Vec<Outcome>) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                outcomes: Mutex::new(outcomes.into()),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl ImapSync for ScriptedSync {
        async fn sync_folder(
            &self,
            _: &str,
            _: u16,
            _: &str,
            _: &str,
            _: &str,
            _: u32,
        ) -> Result<FolderSync, ImapError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self
                .outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Outcome::Ok)
            {
                Outcome::Ok => Ok(FolderSync {
                    uid_validity: Some(1),
                    uid_next: Some(1),
                    exists: 0,
                    messages: vec![],
                }),
                Outcome::Protocol => Err(ImapError::Protocol("temporary failure".into())),
                Outcome::Auth => Err(ImapError::Auth("bad password".into())),
            }
        }
    }

    fn fast_config() -> WorkerConfig {
        WorkerConfig {
            poll_interval: Duration::from_millis(20),
            initial_backoff: Duration::from_millis(5),
            max_backoff: Duration::from_millis(10),
        }
    }

    async fn seed_account_folder(pool: &SqlitePool) -> Uuid {
        let account = accounts::create(
            pool,
            &accounts::NewAccount {
                email: "u@example.com".into(),
                display_name: None,
                auth_kind: AuthKind::Password,
                imap_host: "imap.example.com".into(),
                imap_port: 993,
                imap_use_tls: true,
                smtp_host: "smtp.example.com".into(),
                smtp_port: 465,
                smtp_use_tls: true,
                smtp_starttls: false,
            },
        )
        .await
        .unwrap();
        folders::upsert(
            pool,
            &folders::NewFolder {
                account_id: account.id,
                name: "INBOX".into(),
                delimiter: "/".into(),
                role: FolderRole::Inbox,
                selectable: true,
            },
        )
        .await
        .unwrap();
        account.id
    }

    fn spawn_test_worker(
        pool: SqlitePool,
        sync: Arc<ScriptedSync>,
        account_id: Uuid,
        config: WorkerConfig,
    ) -> (CancellationToken, JoinHandle<()>) {
        let cancel = CancellationToken::new();
        let imap: Arc<dyn ImapSync> = sync;
        let handle = tokio::spawn(run_polling_worker(PollingWorker {
            pool,
            hub: Arc::new(Hub::new()),
            imap,
            account_id,
            folder_name: "INBOX".into(),
            secret: zeroize::Zeroizing::new("right".to_string()),
            cancel: cancel.clone(),
            config,
        }));
        (cancel, handle)
    }

    async fn wait_for_calls(sync: &ScriptedSync, expected: usize) {
        timeout(Duration::from_secs(1), async {
            loop {
                if sync.calls() >= expected {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_worker_polls_at_configured_interval() {
        let pool = crate::db::test_pool().await;
        let account_id = seed_account_folder(&pool).await;
        let sync = Arc::new(ScriptedSync::new(vec![Outcome::Ok]));
        let (cancel, handle) = spawn_test_worker(pool, sync.clone(), account_id, fast_config());

        wait_for_calls(&sync, 3).await;

        cancel.cancel();
        handle.await.unwrap();
        assert!(sync.calls() >= 3);
    }

    #[tokio::test]
    async fn test_worker_stops_on_cancel_and_no_further_syncs_happen() {
        let pool = crate::db::test_pool().await;
        let account_id = seed_account_folder(&pool).await;
        let sync = Arc::new(ScriptedSync::new(vec![Outcome::Ok]));
        let (cancel, handle) = spawn_test_worker(pool, sync.clone(), account_id, fast_config());

        wait_for_calls(&sync, 1).await;
        cancel.cancel();
        timeout(Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap();
        let after_cancel = sync.calls();

        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(sync.calls(), after_cancel);
    }

    #[tokio::test]
    async fn test_worker_recovers_from_transient_protocol_error() {
        let pool = crate::db::test_pool().await;
        let account_id = seed_account_folder(&pool).await;
        let sync = Arc::new(ScriptedSync::new(vec![Outcome::Protocol, Outcome::Ok]));
        let (cancel, handle) = spawn_test_worker(pool, sync.clone(), account_id, fast_config());

        wait_for_calls(&sync, 2).await;

        cancel.cancel();
        handle.await.unwrap();
        assert!(sync.calls() >= 2);
    }

    #[tokio::test]
    async fn test_worker_terminates_on_auth_error() {
        let pool = crate::db::test_pool().await;
        let account_id = seed_account_folder(&pool).await;
        let sync = Arc::new(ScriptedSync::new(vec![Outcome::Auth]));
        let (_cancel, handle) = spawn_test_worker(pool, sync.clone(), account_id, fast_config());

        timeout(Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap();
        let after_auth = sync.calls();

        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(after_auth, 1);
        assert_eq!(sync.calls(), after_auth);
    }
}
