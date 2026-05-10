//! Owns live polling workers and coordinates cancellation.

use std::collections::HashMap;
use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::auth::{CredentialKind, MailCredential};
use crate::imap::{ImapIdle, ImapSync};
use crate::ipc::Hub;
use crate::models::AccountId;

use super::state::{publish_sync_state, SyncState, SyncStateEvent};
use super::worker::{
    run_sync_worker, SyncWorker, WorkerConfig, WorkerCredentialResolver, WorkerCredentialSource,
};

type WorkerKey = (AccountId, String);

pub struct WorkerManager {
    pool: SqlitePool,
    hub: Arc<Hub>,
    imap: Arc<dyn ImapSync>,
    idle: Option<Arc<dyn ImapIdle>>,
    config: WorkerConfig,
    credential_resolver: Option<Arc<dyn WorkerCredentialResolver>>,
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
        Self::with_idle_config(pool, hub, imap, None, config)
    }

    pub fn with_idle_config(
        pool: SqlitePool,
        hub: Arc<Hub>,
        imap: Arc<dyn ImapSync>,
        idle: Option<Arc<dyn ImapIdle>>,
        config: WorkerConfig,
    ) -> Self {
        Self::with_optional_credential_resolver(pool, hub, imap, idle, config, None)
    }

    pub fn with_config_and_credential_resolver(
        pool: SqlitePool,
        hub: Arc<Hub>,
        imap: Arc<dyn ImapSync>,
        config: WorkerConfig,
        credential_resolver: Arc<dyn WorkerCredentialResolver>,
    ) -> Self {
        Self::with_optional_credential_resolver(
            pool,
            hub,
            imap,
            None,
            config,
            Some(credential_resolver),
        )
    }

    pub fn with_idle_config_and_credential_resolver(
        pool: SqlitePool,
        hub: Arc<Hub>,
        imap: Arc<dyn ImapSync>,
        idle: Option<Arc<dyn ImapIdle>>,
        config: WorkerConfig,
        credential_resolver: Arc<dyn WorkerCredentialResolver>,
    ) -> Self {
        Self::with_optional_credential_resolver(
            pool,
            hub,
            imap,
            idle,
            config,
            Some(credential_resolver),
        )
    }

    fn with_optional_credential_resolver(
        pool: SqlitePool,
        hub: Arc<Hub>,
        imap: Arc<dyn ImapSync>,
        idle: Option<Arc<dyn ImapIdle>>,
        config: WorkerConfig,
        credential_resolver: Option<Arc<dyn WorkerCredentialResolver>>,
    ) -> Self {
        Self {
            pool,
            hub,
            imap,
            idle,
            config,
            credential_resolver,
            workers: Mutex::new(HashMap::new()),
        }
    }

    pub async fn start(
        &self,
        account_id: AccountId,
        folder_name: String,
        credential: MailCredential,
    ) -> bool {
        let key = (account_id, folder_name.clone());
        let mut workers = self.workers.lock().await;
        if let Some(existing) = workers.get(&key) {
            if !existing.join.is_finished() {
                return false;
            }
        }
        workers.remove(&key);

        let cancel = CancellationToken::new();
        let credential = self.credential_source(account_id, credential);
        let initial_state = if self.idle.is_some() {
            SyncState::Idle
        } else {
            SyncState::Polling
        };
        publish_sync_state(
            &self.hub,
            SyncStateEvent::new(account_id, initial_state, None),
        )
        .await;
        let join = tokio::spawn(run_sync_worker(SyncWorker {
            pool: self.pool.clone(),
            hub: self.hub.clone(),
            imap: self.imap.clone(),
            idle: self.idle.clone(),
            account_id,
            folder_name,
            credential,
            cancel: cancel.clone(),
            config: self.config,
        }));
        workers.insert(key, WorkerHandle { cancel, join });
        true
    }

    fn credential_source(
        &self,
        account_id: AccountId,
        credential: MailCredential,
    ) -> WorkerCredentialSource {
        if credential.kind() == CredentialKind::OAuth2Bearer {
            if let Some(resolver) = &self.credential_resolver {
                return WorkerCredentialSource::dynamic(account_id, resolver.clone());
            }
        }
        WorkerCredentialSource::static_credential(credential)
    }

    pub async fn stop(&self, account_id: AccountId, folder_name: &str) -> bool {
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
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use crate::db::{accounts, folders};
    use crate::imap::{FolderSync, ImapError};
    use crate::models::{AuthKind, FolderRole};
    use crate::sync::SyncError;

    use super::*;

    struct CountingSync {
        calls: AtomicUsize,
        secrets: Mutex<Vec<String>>,
    }

    impl CountingSync {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                secrets: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn secrets(&self) -> Vec<String> {
            self.secrets.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl ImapSync for CountingSync {
        async fn sync_folder(
            &self,
            _: &str,
            _: u16,
            _: &str,
            credential: &MailCredential,
            _: &str,
            _: u32,
        ) -> Result<FolderSync, ImapError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.secrets
                .lock()
                .unwrap()
                .push(credential.secret().to_string());
            Ok(FolderSync {
                uid_validity: Some(1),
                uid_next: Some(1),
                exists: 0,
                messages: vec![],
            })
        }
    }

    struct ScriptedCredentialResolver {
        calls: AtomicUsize,
        secrets: Mutex<VecDeque<String>>,
    }

    impl ScriptedCredentialResolver {
        fn new(secrets: Vec<&str>) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                secrets: Mutex::new(secrets.into_iter().map(String::from).collect()),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl WorkerCredentialResolver for ScriptedCredentialResolver {
        async fn resolve(&self, _: AccountId) -> Result<MailCredential, SyncError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let secret = self
                .secrets
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| "fresh-token-last".into());
            Ok(MailCredential::oauth2_bearer(secret))
        }
    }

    async fn seed_account_folder(pool: &SqlitePool) -> AccountId {
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
                idle_timeout: Duration::from_secs(30),
                initial_backoff: Duration::from_millis(5),
                max_backoff: Duration::from_millis(10),
            },
        );

        let started = manager
            .start(
                account_id,
                "INBOX".into(),
                MailCredential::password("right"),
            )
            .await;
        let duplicate = manager
            .start(
                account_id,
                "INBOX".into(),
                MailCredential::password("right"),
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
                idle_timeout: Duration::from_secs(30),
                initial_backoff: Duration::from_millis(5),
                max_backoff: Duration::from_millis(10),
            },
        );

        assert!(
            manager
                .start(
                    account_id,
                    "INBOX".into(),
                    MailCredential::password("right"),
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

    #[tokio::test]
    async fn test_oauth_worker_re_resolves_credentials_across_poll_cycles() {
        let pool = crate::db::test_pool().await;
        let account_id = seed_account_folder(&pool).await;
        let sync = Arc::new(CountingSync::new());
        let imap: Arc<dyn ImapSync> = sync.clone();
        let resolver = Arc::new(ScriptedCredentialResolver::new(vec![
            "fresh-token-1",
            "fresh-token-2",
        ]));
        let manager = WorkerManager::with_config_and_credential_resolver(
            pool,
            Arc::new(Hub::new()),
            imap,
            WorkerConfig {
                poll_interval: Duration::from_millis(10),
                idle_timeout: Duration::from_secs(30),
                initial_backoff: Duration::from_millis(5),
                max_backoff: Duration::from_millis(10),
            },
            resolver.clone(),
        );

        assert!(
            manager
                .start(
                    account_id,
                    "INBOX".into(),
                    MailCredential::oauth2_bearer("stale-token"),
                )
                .await
        );
        wait_for_calls(&sync, 2).await;
        manager.stop_all().await;

        assert_eq!(resolver.calls(), 2);
        assert_eq!(
            &sync.secrets()[..2],
            ["fresh-token-1".to_string(), "fresh-token-2".to_string()]
        );
    }

    #[tokio::test]
    async fn test_password_worker_keeps_static_credential_when_resolver_exists() {
        let pool = crate::db::test_pool().await;
        let account_id = seed_account_folder(&pool).await;
        let sync = Arc::new(CountingSync::new());
        let imap: Arc<dyn ImapSync> = sync.clone();
        let resolver = Arc::new(ScriptedCredentialResolver::new(vec!["unexpected-token"]));
        let manager = WorkerManager::with_config_and_credential_resolver(
            pool,
            Arc::new(Hub::new()),
            imap,
            WorkerConfig {
                poll_interval: Duration::from_millis(10),
                idle_timeout: Duration::from_secs(30),
                initial_backoff: Duration::from_millis(5),
                max_backoff: Duration::from_millis(10),
            },
            resolver.clone(),
        );

        assert!(
            manager
                .start(
                    account_id,
                    "INBOX".into(),
                    MailCredential::password("right"),
                )
                .await
        );
        wait_for_calls(&sync, 2).await;
        manager.stop_all().await;

        assert_eq!(resolver.calls(), 0);
        assert!(sync.secrets().iter().all(|secret| secret == "right"));
    }
}
