//! Owns live polling workers and coordinates cancellation.

use std::collections::HashMap;
use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::imap::ImapSync;
use crate::ipc::Hub;
use crate::secrets::Secret;

use super::worker::{run_polling_worker, PollingWorker, WorkerConfig};

type WorkerKey = (Uuid, String);

pub struct WorkerManager {
    pool: SqlitePool,
    hub: Arc<Hub>,
    imap: Arc<dyn ImapSync>,
    config: WorkerConfig,
    workers: Mutex<HashMap<WorkerKey, WorkerHandle>>,
}

struct WorkerHandle {
    cancel: CancellationToken,
    join: JoinHandle<()>,
}

impl WorkerManager {
    pub fn new(pool: SqlitePool, hub: Arc<Hub>, imap: Arc<dyn ImapSync>) -> Self {
        Self::with_config(pool, hub, imap, WorkerConfig::default())
    }

    pub fn with_config(
        pool: SqlitePool,
        hub: Arc<Hub>,
        imap: Arc<dyn ImapSync>,
        config: WorkerConfig,
    ) -> Self {
        Self {
            pool,
            hub,
            imap,
            config,
            workers: Mutex::new(HashMap::new()),
        }
    }

    pub async fn start(&self, account_id: Uuid, folder_name: String, secret: Secret) -> bool {
        let key = (account_id, folder_name.clone());
        let mut workers = self.workers.lock().await;
        if let Some(existing) = workers.get(&key) {
            if !existing.join.is_finished() {
                return false;
            }
        }
        workers.remove(&key);

        let cancel = CancellationToken::new();
        let join = tokio::spawn(run_polling_worker(PollingWorker {
            pool: self.pool.clone(),
            hub: self.hub.clone(),
            imap: self.imap.clone(),
            account_id,
            folder_name,
            secret,
            cancel: cancel.clone(),
            config: self.config,
        }));
        workers.insert(key, WorkerHandle { cancel, join });
        true
    }

    pub async fn stop(&self, account_id: Uuid, folder_name: &str) -> bool {
        let key = (account_id, folder_name.to_string());
        let handle = self.workers.lock().await.remove(&key);
        if let Some(handle) = handle {
            handle.cancel.cancel();
            await_worker(handle.join).await;
            true
        } else {
            false
        }
    }

    pub async fn stop_all(&self) {
        let handles = {
            let mut workers = self.workers.lock().await;
            workers
                .drain()
                .map(|(_, handle)| handle)
                .collect::<Vec<_>>()
        };

        for handle in &handles {
            handle.cancel.cancel();
        }
        for handle in handles {
            await_worker(handle.join).await;
        }
    }
}

async fn await_worker(join: JoinHandle<()>) {
    if let Err(e) = join.await {
        tracing::warn!(error = %e, "sync worker join failed");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use crate::db::{accounts, folders};
    use crate::imap::{FolderSync, ImapError};
    use crate::models::{AuthKind, FolderRole};

    use super::*;

    struct CountingSync {
        calls: AtomicUsize,
    }

    impl CountingSync {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl ImapSync for CountingSync {
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
            Ok(FolderSync {
                uid_validity: Some(1),
                uid_next: Some(1),
                exists: 0,
                messages: vec![],
            })
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

    async fn wait_for_calls(sync: &CountingSync, expected: usize) {
        tokio::time::timeout(Duration::from_secs(1), async {
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
    async fn test_manager_start_is_idempotent_does_not_spawn_two_workers() {
        let pool = crate::db::test_pool().await;
        let account_id = seed_account_folder(&pool).await;
        let sync = Arc::new(CountingSync::new());
        let imap: Arc<dyn ImapSync> = sync.clone();
        let manager = WorkerManager::with_config(
            pool,
            Arc::new(Hub::new()),
            imap,
            WorkerConfig {
                poll_interval: Duration::from_secs(30),
                initial_backoff: Duration::from_millis(5),
                max_backoff: Duration::from_millis(10),
            },
        );

        let started = manager
            .start(
                account_id,
                "INBOX".into(),
                zeroize::Zeroizing::new("right".to_string()),
            )
            .await;
        let duplicate = manager
            .start(
                account_id,
                "INBOX".into(),
                zeroize::Zeroizing::new("right".to_string()),
            )
            .await;

        assert!(started);
        assert!(!duplicate);
        wait_for_calls(&sync, 1).await;
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(sync.calls(), 1);

        manager.stop_all().await;
    }

    #[tokio::test]
    async fn test_manager_stop_removes_and_cancels_worker() {
        let pool = crate::db::test_pool().await;
        let account_id = seed_account_folder(&pool).await;
        let sync = Arc::new(CountingSync::new());
        let imap: Arc<dyn ImapSync> = sync.clone();
        let manager = WorkerManager::with_config(
            pool,
            Arc::new(Hub::new()),
            imap,
            WorkerConfig {
                poll_interval: Duration::from_millis(10),
                initial_backoff: Duration::from_millis(5),
                max_backoff: Duration::from_millis(10),
            },
        );

        assert!(
            manager
                .start(
                    account_id,
                    "INBOX".into(),
                    zeroize::Zeroizing::new("right".to_string()),
                )
                .await
        );
        wait_for_calls(&sync, 1).await;

        assert!(manager.stop(account_id, "INBOX").await);
        assert!(manager.workers.lock().await.is_empty());
        let after_stop = sync.calls();

        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(sync.calls(), after_stop);
        assert!(!manager.stop(account_id, "INBOX").await);
    }
}
